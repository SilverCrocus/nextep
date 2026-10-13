//! New-episode alerts: hands the plan from `nextep_core::alerts` to Android's
//! notification scheduler (through the notification plugin).

use nextep_core::alerts::Alert;
use tauri::plugin::PermissionState;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_notification::NotificationExt;

use crate::state::AppState;

#[cfg(target_os = "android")]
const CHANNEL: &str = "new_episodes";

/// Schedules the current plan in place of the last one. Does nothing when
/// the plan hasn't changed, so it's cheap to call after every refresh.
pub async fn reschedule<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    let plan = state.alert_plan().await;
    if !state.note_scheduled(&plan).await {
        return;
    }
    let app = app.clone();
    // The plugin's calls block on the Android side.
    let _ = tauri::async_runtime::spawn_blocking(move || schedule(&app, &plan)).await;
}

#[cfg(target_os = "android")]
fn schedule<R: Runtime>(
    app: &AppHandle<R>,
    plan: &[Alert],
) -> tauri_plugin_notification::Result<()> {
    use tauri_plugin_notification::{Channel, Importance, Schedule};

    let notifications = app.notification();
    // Only alerts are ever scheduled, so this clears the previous plan.
    notifications.cancel_all()?;
    if plan.is_empty() {
        return Ok(());
    }
    notifications.create_channel(
        Channel::builder(CHANNEL, "New episodes")
            .description("When a new episode of a show you're caught up on airs")
            .importance(Importance::Default)
            .build(),
    )?;
    for alert in plan {
        let Ok(at) = time::OffsetDateTime::from_unix_timestamp(alert.at.timestamp()) else {
            continue;
        };
        notifications
            .builder()
            .id(alert.id)
            .channel_id(CHANNEL)
            .title(&alert.title)
            .body(&alert.body)
            .auto_cancel()
            .schedule(Schedule::At {
                date: at,
                repeating: false,
                allow_while_idle: true,
            })
            .show()?;
    }
    Ok(())
}

/// Scheduling needs Android; elsewhere there's nothing to do.
#[cfg(not(target_os = "android"))]
fn schedule<R: Runtime>(
    _app: &AppHandle<R>,
    _plan: &[Alert],
) -> tauri_plugin_notification::Result<()> {
    Ok(())
}

/// Whether the app may post notifications, asking the user if it hasn't yet.
pub async fn ask_permission<R: Runtime>(app: &AppHandle<R>) -> bool {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let notifications = app.notification();
        match notifications.permission_state() {
            Ok(PermissionState::Granted) => true,
            _ => matches!(
                notifications.request_permission(),
                Ok(PermissionState::Granted)
            ),
        }
    })
    .await
    .unwrap_or(false)
}

/// Whether the app may post notifications, without asking.
pub async fn permitted<R: Runtime>(app: &AppHandle<R>) -> bool {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        matches!(
            app.notification().permission_state(),
            Ok(PermissionState::Granted)
        )
    })
    .await
    .unwrap_or(false)
}
