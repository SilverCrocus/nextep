package io.github.silvercrocus.nextep

import android.app.Activity
import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.text.format.DateUtils
import android.view.View
import android.widget.RemoteViews
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin
import org.json.JSONArray

/**
 * The "Up next" home-screen widget. Nextep hands it the rows to show (see
 * nextep_core::widget) whenever Progress refreshes; they're kept in
 * SharedPreferences so the widget can redraw without the app running.
 */
class NextepWidget : AppWidgetProvider() {
  override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
    val views = views(context)
    ids.forEach { manager.updateAppWidget(it, views) }
  }

  companion object {
    private const val PREFS = "nextep_widget"
    private const val ROWS = "rows"
    private val rowIds = intArrayOf(R.id.row0, R.id.row1, R.id.row2, R.id.row3)
    private val titleIds = intArrayOf(R.id.title0, R.id.title1, R.id.title2, R.id.title3)
    private val lineIds = intArrayOf(R.id.line0, R.id.line1, R.id.line2, R.id.line3)

    /** Keeps the rows (JSON from Rust) and redraws every widget. */
    fun save(context: Context, rows: String) {
      context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(ROWS, rows).apply()
      val manager = AppWidgetManager.getInstance(context)
      val ids = manager.getAppWidgetIds(ComponentName(context, NextepWidget::class.java))
      if (ids.isNotEmpty()) {
        val views = views(context)
        ids.forEach { manager.updateAppWidget(it, views) }
      }
    }

    private fun views(context: Context): RemoteViews {
      val views = RemoteViews(context.packageName, R.layout.nextep_widget)
      val json = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(ROWS, null)
      val rows = runCatching { JSONArray(json ?: "[]") }.getOrDefault(JSONArray())
      for (i in rowIds.indices) {
        val row = rows.optJSONObject(i)
        if (row == null) {
          views.setViewVisibility(rowIds[i], View.GONE)
          continue
        }
        views.setViewVisibility(rowIds[i], View.VISIBLE)
        views.setTextViewText(titleIds[i], row.optString("title"))
        views.setTextViewText(lineIds[i], line(context, row))
      }
      views.setViewVisibility(R.id.empty, if (rows.length() == 0) View.VISIBLE else View.GONE)

      // Tapping the widget opens Nextep.
      context.packageManager.getLaunchIntentForPackage(context.packageName)?.let { launch ->
        val open =
          PendingIntent.getActivity(
            context,
            0,
            launch,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
          )
        views.setOnClickPendingIntent(R.id.root, open)
      }
      return views
    }

    /** "S01E04 · The Faceless · 2 left" or "… · Fri, 16 Oct, 6:00 pm". */
    private fun line(context: Context, row: org.json.JSONObject): String {
      val parts = mutableListOf(row.optString("code"))
      row.optString("episode").takeIf { it.isNotEmpty() && !row.isNull("episode") }?.let { parts.add(it) }
      when {
        !row.isNull("airsAt") -> parts.add(
          DateUtils.formatDateTime(
            context,
            row.getLong("airsAt"),
            DateUtils.FORMAT_SHOW_WEEKDAY or DateUtils.FORMAT_ABBREV_WEEKDAY or
              DateUtils.FORMAT_SHOW_DATE or DateUtils.FORMAT_ABBREV_MONTH or
              DateUtils.FORMAT_SHOW_TIME
          )
        )
        !row.isNull("left") -> parts.add("${row.getInt("left")} left")
      }
      return parts.joinToString(" · ")
    }
  }
}

@InvokeArg
class WidgetArgs {
  /** The rows as JSON (nextep_core::widget::WidgetRow). */
  var rows: String = "[]"
}

/** Lets Rust update the widget. Registered in src-tauri/src/widget.rs. */
@TauriPlugin
class WidgetPlugin(private val activity: Activity) : Plugin(activity) {
  @Command
  fun update(invoke: Invoke) {
    NextepWidget.save(activity.applicationContext, invoke.parseArgs(WidgetArgs::class.java).rows)
    invoke.resolve()
  }
}
