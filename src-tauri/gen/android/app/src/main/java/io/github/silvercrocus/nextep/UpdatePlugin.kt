package io.github.silvercrocus.nextep

import android.app.Activity
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import androidx.core.content.IntentCompat
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File

@InvokeArg
class InstallArgs {
  /** Absolute path of a downloaded APK inside the app's cache directory. */
  var path: String = ""
}

/**
 * Installs downloaded updates with Android's PackageInstaller. Registered
 * from Rust in src-tauri/src/installer.rs.
 *
 * On Android 12+ an app may update itself without the system's confirmation
 * (it asks for that with USER_ACTION_NOT_REQUIRED and declares
 * UPDATE_PACKAGES_WITHOUT_USER_ACTION), unless another installer such as
 * Obtainium owns its updates (Android 14+). Updates found while Nextep is
 * open are installed a few seconds after it's left, so it never closes
 * under the user.
 */
@TauriPlugin
class UpdatePlugin(private val activity: Activity) : Plugin(activity) {
  /** A downloaded update waiting for the app to be left. */
  private var waiting: File? = null
  private val handler = Handler(Looper.getMainLooper())
  private val installLeft = Runnable {
    val file = waiting ?: return@Runnable
    waiting = null
    if (canInstallSilently()) {
      val context = activity.applicationContext
      Thread { runCatching { commit(context, file) } }.start()
    }
  }

  /**
   * Whether Nextep may install updates, and without a confirmation; and the
   * app that owns its updates, if another one does.
   */
  @Command
  fun status(invoke: Invoke) {
    invoke.resolve(
      JSObject()
        .put("allowed", canInstall())
        .put("silent", canInstallSilently())
        .put("owner", otherUpdateOwner())
    )
  }

  /** Android's "Install unknown apps" switch for Nextep. */
  @Command
  fun openInstallSettings(invoke: Invoke) {
    openSettings()
    invoke.resolve()
  }

  /** Installs now (the user tapped Update). */
  @Command
  fun install(invoke: Invoke) {
    val file = File(invoke.parseArgs(InstallArgs::class.java).path)
    // This replaces any install waiting for the app to be left.
    cancelWaiting()
    if (!file.exists()) {
      invoke.reject("The downloaded update is missing.")
      return
    }
    // Android asks once per app before it may install updates.
    if (!canInstall()) {
      openSettings()
      invoke.resolve(JSObject().put("needsPermission", true))
      return
    }
    // Copying the APK into the session takes a moment; not on the UI thread.
    val context = activity.applicationContext
    Thread {
      try {
        commit(context, file)
        invoke.resolve(JSObject().put("needsPermission", false))
      } catch (e: Exception) {
        invoke.reject("Couldn't start the update: ${e.message}")
      }
    }.start()
  }

  /** Installs once the app is left, if that needs no confirmation. */
  @Command
  fun installWhenLeft(invoke: Invoke) {
    val file = File(invoke.parseArgs(InstallArgs::class.java).path)
    waiting = file.takeIf { it.exists() }
    invoke.resolve()
  }

  /** Forgets an install waiting for the app to be left. */
  @Command
  fun cancelWhenLeft(invoke: Invoke) {
    cancelWaiting()
    invoke.resolve()
  }

  override fun onResume() {
    foreground = true
    // Back before the install started: keep waiting.
    handler.removeCallbacks(installLeft)
  }

  override fun onPause() {
    foreground = false
  }

  override fun onStop() {
    // Recreated for a configuration change, not left.
    if (waiting == null || activity.isChangingConfigurations) return
    handler.postDelayed(installLeft, LEAVE_DELAY_MS)
  }

  private fun cancelWaiting() {
    waiting = null
    handler.removeCallbacks(installLeft)
  }

  private fun canInstall(): Boolean =
    Build.VERSION.SDK_INT < Build.VERSION_CODES.O ||
      activity.packageManager.canRequestPackageInstalls()

  private fun canInstallSilently(): Boolean =
    Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && canInstall() && otherUpdateOwner() == null

  /** Another app that owns Nextep's updates (Android 14+), e.g. Obtainium. */
  private fun otherUpdateOwner(): String? {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) return null
    val owner =
      runCatching {
        activity.packageManager.getInstallSourceInfo(activity.packageName).updateOwnerPackageName
      }.getOrNull()
    return owner?.takeIf { it != activity.packageName }
  }

  private fun openSettings() {
    activity.startActivity(
      Intent(
        Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
        Uri.parse("package:${activity.packageName}")
      )
    )
  }

  companion object {
    /** Whether Nextep is on screen, for the install result. */
    @Volatile
    var foreground = false

    private const val LEAVE_DELAY_MS = 5_000L

    fun commit(context: Context, file: File) {
      val installer = context.packageManager.packageInstaller
      val params =
        PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
          setAppPackageName(context.packageName)
          if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
          }
        }
      val id = installer.createSession(params)
      try {
        installer.openSession(id).use { session ->
          file.inputStream().use { input ->
            session.openWrite("nextep.apk", 0, file.length()).use { output ->
              input.copyTo(output)
              session.fsync(output)
            }
          }
          // The system fills in the result, so the intent must be mutable.
          val flags =
            PendingIntent.FLAG_UPDATE_CURRENT or
              (if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) PendingIntent.FLAG_MUTABLE else 0)
          val result =
            PendingIntent.getBroadcast(
              context,
              id,
              Intent(context, UpdateStatusReceiver::class.java),
              flags
            )
          session.commit(result.intentSender)
        }
      } catch (e: Exception) {
        runCatching { installer.abandonSession(id) }
        throw e
      }
    }
  }
}

/**
 * The result of an update install. Shows Android's confirmation when it
 * needs one and Nextep is on screen; otherwise drops the install, and the
 * banner offers it again next time.
 */
class UpdateStatusReceiver : BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent) {
    if (intent.getIntExtra(PackageInstaller.EXTRA_STATUS, 0) !=
      PackageInstaller.STATUS_PENDING_USER_ACTION
    ) {
      // Installed (this process is being replaced) or failed.
      return
    }
    val sessionId = intent.getIntExtra(PackageInstaller.EXTRA_SESSION_ID, -1)
    val confirm = IntentCompat.getParcelableExtra(intent, Intent.EXTRA_INTENT, Intent::class.java)
    val shown =
      UpdatePlugin.foreground && confirm != null &&
        runCatching {
          context.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        }.isSuccess
    if (!shown && sessionId != -1) {
      runCatching { context.packageManager.packageInstaller.abandonSession(sessionId) }
    }
  }
}
