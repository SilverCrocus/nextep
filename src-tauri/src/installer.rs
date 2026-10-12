//! In-app updates: download the release APK and hand it to Android's package
//! installer (via UpdatePlugin.kt), instead of relying on a browser download.

use std::fs;

use serde::Deserialize;
use tauri::plugin::{Builder, TauriPlugin};
use tauri::{AppHandle, Manager, Runtime};

use crate::state::to_ui_error;

/// Only Nextep's own release assets may be installed this way.
const RELEASES: &str = "https://github.com/SilverCrocus/nextep/releases/download/";

#[cfg(target_os = "android")]
struct Installer<R: Runtime>(tauri::plugin::PluginHandle<R>);

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("installer")
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                let handle =
                    _api.register_android_plugin("io.github.silvercrocus.nextep", "UpdatePlugin")?;
                _app.manage(Installer(handle));
            }
            Ok(())
        })
        .build()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct InstallResult {
    needs_permission: bool,
}

/// Downloads the APK and opens Android's installer. Returns `false` when the
/// user first has to allow Nextep to install apps (Android shows that
/// setting); they then tap Update again.
#[tauri::command]
pub async fn install_update(app: AppHandle, url: String) -> Result<bool, String> {
    if !url.starts_with(RELEASES) || !url.ends_with(".apk") {
        return Err("Not a Nextep release.".into());
    }
    let apk = nextep_core::update::download(&url)
        .await
        .map_err(to_ui_error)?;
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("updates");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("nextep-update.apk");
    fs::write(&path, apk).map_err(|e| e.to_string())?;
    open_installer(&app, &path)
}

#[cfg(target_os = "android")]
fn open_installer<R: Runtime>(app: &AppHandle<R>, path: &std::path::Path) -> Result<bool, String> {
    let installer = app.state::<Installer<R>>();
    let result: InstallResult = installer
        .0
        .run_mobile_plugin("install", serde_json::json!({ "path": path }))
        .map_err(|e| e.to_string())?;
    Ok(!result.needs_permission)
}

#[cfg(not(target_os = "android"))]
fn open_installer<R: Runtime>(
    _app: &AppHandle<R>,
    _path: &std::path::Path,
) -> Result<bool, String> {
    Err("In-app updates are only available on Android.".into())
}
