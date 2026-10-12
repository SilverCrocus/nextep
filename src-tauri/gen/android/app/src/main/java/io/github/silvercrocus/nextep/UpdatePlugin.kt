package io.github.silvercrocus.nextep

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.core.content.FileProvider
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
 * Hands a downloaded update to Android's package installer, so updating
 * doesn't depend on a browser's download handling. Registered from Rust in
 * src-tauri/src/installer.rs.
 */
@TauriPlugin
class UpdatePlugin(private val activity: Activity) : Plugin(activity) {
  @Command
  fun install(invoke: Invoke) {
    val args = invoke.parseArgs(InstallArgs::class.java)
    val file = File(args.path)
    if (!file.exists()) {
      invoke.reject("The downloaded update is missing.")
      return
    }

    // Android asks once per app before it may install updates.
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
      !activity.packageManager.canRequestPackageInstalls()
    ) {
      val settings = Intent(
        Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
        Uri.parse("package:${activity.packageName}")
      )
      activity.startActivity(settings)
      invoke.resolve(JSObject().put("needsPermission", true))
      return
    }

    val uri = FileProvider.getUriForFile(activity, "${activity.packageName}.fileprovider", file)
    val install = Intent(Intent.ACTION_VIEW).apply {
      setDataAndType(uri, "application/vnd.android.package-archive")
      addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
    }
    activity.startActivity(install)
    invoke.resolve(JSObject().put("needsPermission", false))
  }
}
