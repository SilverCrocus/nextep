//! In-app updates: download the release APK and install it with Android's
//! package installer (via UpdatePlugin.kt): without a confirmation where
//! Android 12+ allows Nextep to update itself, else with its installer
//! screen.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::async_runtime::Mutex;
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

/// Whether Nextep may install updates (Android's "Install unknown apps"
/// switch), and whether it can do so without a confirmation (Android 12+);
/// `owner` is another app that owns its updates (e.g. Obtainium), which
/// rules that out.
#[derive(Default, Serialize, Deserialize)]
pub struct UpdateStatus {
    allowed: bool,
    silent: bool,
    owner: Option<String>,
}

/// Downloads the APK and installs it: without a confirmation where Android
/// allows, else with its installer screen. Returns `false` when the user
/// first has to allow Nextep to install apps (Android shows that setting);
/// they then tap Update again.
#[tauri::command]
pub async fn install_update(app: AppHandle, url: String) -> Result<bool, String> {
    let path = download(&app, &url).await?;
    let result: InstallResult = run(&app, "install", json!({ "path": path }))?;
    Ok(!result.needs_permission)
}

/// Downloads the APK now and installs it once Nextep is left, if that needs
/// no confirmation.
#[tauri::command]
pub async fn install_when_left(app: AppHandle, url: String) -> Result<(), String> {
    let path = download(&app, &url).await?;
    run::<serde_json::Value>(&app, "installWhenLeft", json!({ "path": path }))?;
    Ok(())
}

/// Forgets an update waiting for the app to be left.
#[tauri::command]
pub fn cancel_when_left(app: AppHandle) -> Result<(), String> {
    run::<serde_json::Value>(&app, "cancelWhenLeft", json!({}))?;
    Ok(())
}

#[tauri::command]
pub fn update_status(app: AppHandle) -> UpdateStatus {
    run(&app, "status", json!({})).unwrap_or_default()
}

/// Opens Android's "Install unknown apps" switch for Nextep.
#[tauri::command]
pub fn open_install_settings(app: AppHandle) -> Result<(), String> {
    run::<serde_json::Value>(&app, "openInstallSettings", json!({}))?;
    Ok(())
}

/// The release's APK in the cache, downloading it unless it's there from an
/// earlier launch. One download at a time, written to a temporary file
/// first, so an install never reads a half-written APK.
async fn download<R: Runtime>(app: &AppHandle<R>, url: &str) -> Result<PathBuf, String> {
    static DOWNLOADING: Mutex<()> = Mutex::const_new(());

    let name = url
        .strip_prefix(RELEASES)
        .and_then(|rest| rest.rsplit('/').next())
        .filter(|name| name.starts_with("nextep-") && name.ends_with(".apk"))
        .ok_or("Not a Nextep release.")?;
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("updates");
    let path = dir.join(name);
    let _one_at_a_time = DOWNLOADING.lock().await;
    if fs::metadata(&path).is_ok_and(|m| m.len() > 0) {
        return Ok(path);
    }
    let apk = nextep_core::update::download(url)
        .await
        .map_err(to_ui_error)?;
    // Older releases' APKs aren't needed any more.
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let part = dir.join(format!("{name}.part"));
    fs::write(&part, apk).map_err(|e| e.to_string())?;
    fs::rename(&part, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(target_os = "android")]
fn run<T: serde::de::DeserializeOwned>(
    app: &AppHandle,
    command: &str,
    args: serde_json::Value,
) -> Result<T, String> {
    app.state::<Installer<tauri::Wry>>()
        .0
        .run_mobile_plugin(command, args)
        .map_err(|e| e.to_string())
}

#[cfg(not(target_os = "android"))]
fn run<T>(_app: &AppHandle, _command: &str, _args: serde_json::Value) -> Result<T, String> {
    Err("In-app updates are only available on Android.".into())
}
