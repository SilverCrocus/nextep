//! The "Up next" home-screen widget: the rows from `nextep_core::widget`,
//! handed to NextepWidget.kt (through WidgetPlugin), which keeps them and
//! redraws the widgets.

use nextep_core::progress::ProgressView;
use tauri::plugin::{Builder, TauriPlugin};
use tauri::{AppHandle, Runtime};

#[cfg(target_os = "android")]
struct Widget<R: Runtime>(tauri::plugin::PluginHandle<R>);

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("widget")
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                use tauri::Manager;
                let handle =
                    _api.register_android_plugin("io.github.silvercrocus.nextep", "WidgetPlugin")?;
                _app.manage(Widget(handle));
            }
            Ok(())
        })
        .build()
}

/// Shows `view`'s next episodes on the widget; an empty view clears it
/// (signed out).
pub async fn update<R: Runtime>(app: &AppHandle<R>, view: &ProgressView) {
    let rows = nextep_core::widget::rows(view);
    let Ok(rows) = serde_json::to_string(&rows) else {
        return;
    };
    let app = app.clone();
    // The plugin call blocks on the Android side.
    let _ = tauri::async_runtime::spawn_blocking(move || send(&app, rows)).await;
}

#[cfg(target_os = "android")]
fn send<R: Runtime>(app: &AppHandle<R>, rows: String) {
    use tauri::Manager;
    if let Some(widget) = app.try_state::<Widget<R>>() {
        let _: Result<serde_json::Value, _> = widget
            .0
            .run_mobile_plugin("update", serde_json::json!({ "rows": rows }));
    }
}

#[cfg(not(target_os = "android"))]
fn send<R: Runtime>(_app: &AppHandle<R>, _rows: String) {}
