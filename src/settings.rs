//! Settings page: new-episode alerts, updates, and the AI provider and key
//! for AI picks.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};

use crate::api;
use crate::app::nav;

#[derive(Clone, Deserialize)]
pub struct AiSettings {
    pub configured: bool,
    pub base_url: String,
    pub model: String,
    pub reasoning_effort: String,
    pub key_hint: Option<String>,
}

pub async fn load_ai_settings() -> Result<AiSettings, String> {
    api::call("ai_settings", ()).await
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    let nav = nav();
    let base_url = RwSignal::new(String::new());
    let model = RwSignal::new(String::new());
    let effort = RwSignal::new(String::new());
    let key = RwSignal::new(String::new());
    let saved = RwSignal::new(None::<AiSettings>);
    let models = RwSignal::new(Vec::<String>::new());
    let status = RwSignal::new(None::<(bool, String)>);
    let busy = RwSignal::new(false);
    let confirm_remove = RwSignal::new(false);

    let reload = move || {
        spawn_local(async move {
            if let Ok(s) = load_ai_settings().await {
                base_url.set(s.base_url.clone());
                model.set(s.model.clone());
                effort.set(s.reasoning_effort.clone());
                saved.set(Some(s));
            }
        });
    };
    reload();

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct KeyArgs {
        base_url: String,
        api_key: Option<String>,
    }
    let new_key = move || Some(key.get_untracked()).filter(|k| !k.trim().is_empty());

    let check = move |_| {
        busy.set(true);
        status.set(None);
        let args = KeyArgs {
            base_url: base_url.get_untracked(),
            api_key: new_key(),
        };
        spawn_local(async move {
            match api::call::<Vec<String>>("ai_models", args).await {
                Ok(list) => {
                    status.set(Some((true, format!("Key works · {} models", list.len()))));
                    if !list.contains(&model.get_untracked())
                        && let Some(first) = list.first()
                    {
                        model.set(first.clone());
                    }
                    models.set(list);
                }
                Err(e) => status.set(Some((false, e))),
            }
            busy.set(false);
        });
    };

    let save = move |_| {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Args {
            base_url: String,
            model: String,
            reasoning_effort: String,
            api_key: Option<String>,
        }
        busy.set(true);
        let args = Args {
            base_url: base_url.get_untracked(),
            model: model.get_untracked(),
            reasoning_effort: effort.get_untracked(),
            api_key: new_key(),
        };
        spawn_local(async move {
            match api::call::<()>("save_ai_settings", args).await {
                Ok(()) => {
                    key.set(String::new());
                    status.set(Some((true, "Saved".into())));
                    nav.data_changed();
                    reload();
                }
                Err(e) => status.set(Some((false, e))),
            }
            busy.set(false);
        });
    };

    let remove = move |_| {
        if !confirm_remove.get_untracked() {
            confirm_remove.set(true);
            return;
        }
        spawn_local(async move {
            let _ = api::call::<()>("clear_ai_key", ()).await;
            confirm_remove.set(false);
            models.set(Vec::new());
            status.set(Some((true, "Key removed".into())));
            nav.data_changed();
            reload();
        });
    };

    view! {
        <div class="sheet">
            <button class="back" aria-label="Back" on:click=move |_| nav.close()>
                "←"
            </button>
            <div class="details list-page">
                <h1 class="show-title">"Settings"</h1>

                <Alerts/>
                <Updates/>

                <h2 class="section">"AI picks"</h2>
                <p class="hint small">
                    "Describe a mood in Discover and an AI suggests shows or movies for you. "
                    "Works with DeepSeek or any OpenAI-compatible API. Your key stays on this phone; "
                    "each request sends your mood, titles you watched, liked and dropped, people from shows you liked, "
                    "and the candidates to the provider."
                </p>

                <label class="field">
                    <span class="field-label">"Provider URL"</span>
                    <input
                        type="url"
                        autocomplete="off"
                        prop:value=move || base_url.get()
                        on:input=move |ev| base_url.set(event_target_value(&ev))
                    />
                </label>

                <label class="field">
                    <span class="field-label">"API key"</span>
                    <input
                        type="password"
                        autocomplete="off"
                        placeholder=move || match saved.get().and_then(|s| s.key_hint) {
                            Some(hint) => format!("Saved key ••••{hint} (paste to replace)"),
                            None => "Paste your API key".into(),
                        }
                        prop:value=move || key.get()
                        on:input=move |ev| key.set(event_target_value(&ev))
                    />
                </label>

                <label class="field">
                    <span class="field-label">"Model"</span>
                    {move || {
                        let list = models.get();
                        if list.is_empty() {
                            view! {
                                <input
                                    type="text"
                                    autocomplete="off"
                                    prop:value=move || model.get()
                                    on:input=move |ev| model.set(event_target_value(&ev))
                                />
                            }
                                .into_any()
                        } else {
                            view! {
                                <select
                                    prop:value=move || model.get()
                                    on:change=move |ev| model.set(event_target_value(&ev))
                                >
                                    {list.into_iter().map(|m| {
                                            let label = m.clone();
                                            view! { <option value=m>{label}</option> }
                                        }).collect_view()}
                                </select>
                            }
                                .into_any()
                        }
                    }}
                </label>

                <label class="field">
                    <span class="field-label">"Thinking"</span>
                    <select prop:value=move || effort.get() on:change=move |ev| effort.set(event_target_value(&ev))>
                        <option value="none">"Off (fastest, a few seconds)"</option>
                        <option value="low">"Low"</option>
                        <option value="high">"High"</option>
                        <option value="max">"Max (slowest, a minute or more)"</option>
                        <option value="">"Provider default"</option>
                    </select>
                </label>

                <div class="actions">
                    <button class="pill" on:click=check disabled=move || busy.get()>"Check key"</button>
                    <button class="pill on" on:click=save disabled=move || busy.get()>"Save"</button>
                    {move || {
                        saved.get().is_some_and(|s| s.configured).then(|| {
                            view! {
                                <button class="pill danger" on:click=remove>
                                    {move || if confirm_remove.get() { "Tap again to remove" } else { "Remove key" }}
                                </button>
                            }
                        })
                    }}
                </div>
                {move || {
                    status.get().map(|(ok, text)| {
                        view! { <p class=if ok { "hint" } else { "warn" }>{text}</p> }
                    })
                }}
            </div>
        </div>
    }
}

#[derive(Clone, Deserialize)]
struct AlertsView {
    enabled: bool,
    permitted: bool,
}

/// New-episode alerts on or off. Turning them on asks Android for permission.
#[component]
fn Alerts() -> impl IntoView {
    let view = RwSignal::new(None::<AlertsView>);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    spawn_local(async move {
        if let Ok(v) = api::call::<AlertsView>("alerts", ()).await {
            view.set(Some(v));
        }
    });
    let toggle = move |_| {
        #[derive(Serialize)]
        struct Args {
            enabled: bool,
        }
        let enabled = !view.with_untracked(|v| v.as_ref().is_some_and(|v| v.enabled));
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match api::call::<AlertsView>("set_alerts", Args { enabled }).await {
                Ok(v) => view.set(Some(v)),
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        });
    };
    let on = move || view.with(|v| v.as_ref().is_some_and(|v| v.enabled));

    view! {
        <h2 class="section">"New episode alerts"</h2>
        <p class="hint small">
            "A notification when a new episode of a show you're caught up on airs. "
            "They're scheduled when Progress refreshes, so open Nextep now and then to keep them coming."
        </p>
        <div class="actions">
            <button class="pill" class:on=on disabled=move || busy.get() || view.with(Option::is_none) on:click=toggle>
                {move || if on() { "✓ Alerts on" } else { "Turn on alerts" }}
            </button>
        </div>
        {move || {
            view.with(|v| v.as_ref().is_some_and(|v| v.enabled && !v.permitted))
                .then(|| {
                    view! {
                        <p class="warn">
                            "Notifications are blocked for Nextep. Allow them in Settings › Apps › Nextep › Notifications."
                        </p>
                    }
                })
        }}
        {move || error.get().map(|e| view! { <p class="warn">{e}</p> })}
    }
}

/// The version, "Check for updates", and installing updates on their own.
#[component]
fn Updates() -> impl IntoView {
    let nav = nav();
    let status = RwSignal::new(None::<api::UpdateStatus>);
    let auto = RwSignal::new(api::auto_update());
    let checking = RwSignal::new(false);
    // `Some(None)`: up to date.
    let found = RwSignal::new(None::<Option<api::Release>>);
    let note = RwSignal::new(None::<String>);

    // Again on coming back from Android's settings.
    Effect::new(move |_| {
        nav.changed.track();
        spawn_local(async move { status.set(Some(api::update_status().await)) });
    });

    let check = move |_| {
        checking.set(true);
        note.set(None);
        spawn_local(async move {
            match api::call::<Option<api::Release>>("check_update", ()).await {
                Ok(release) => found.set(Some(release)),
                Err(e) => note.set(Some(e)),
            }
            checking.set(false);
        });
    };
    let install = move |url: String| {
        #[derive(Serialize)]
        struct Args {
            url: String,
        }
        note.set(Some("Downloading…".into()));
        spawn_local(async move {
            match api::call::<bool>("install_update", Args { url }).await {
                Ok(true) => note.set(Some("Installing… Nextep will close and update.".into())),
                Ok(false) => note.set(Some(
                    "Allow Nextep to install updates, then come back and tap Update again.".into(),
                )),
                Err(e) => note.set(Some(e)),
            }
        });
    };
    let toggle_auto = move |_| {
        let on = !auto.get_untracked();
        api::set_auto_update(on);
        auto.set(on);
        if !on {
            spawn_local(api::cancel_auto_install());
        }
    };
    let allow = move |_| {
        spawn_local(async move {
            let _ = api::call::<()>("open_install_settings", ()).await;
        })
    };

    view! {
        <h2 class="section">"Updates"</h2>
        <p class="meta">{move || format!("Nextep v{}", nav.version.get())}</p>
        <div class="actions">
            <button class="pill" disabled=move || checking.get() on:click=check>
                {move || if checking.get() { "Checking…" } else { "Check for updates" }}
            </button>
        </div>
        {move || {
            found.get().map(|release| match release {
                None => view! { <p class="hint">"You're on the latest version."</p> }.into_any(),
                Some(release) => {
                    let url = release.apk_url.clone();
                    view! {
                        <p class="hint">{format!("Nextep {} is available.", release.version)}</p>
                        {url.map(|url| {
                            view! {
                                <button class="pill on" on:click=move |_| install(url.clone())>
                                    "Update now"
                                </button>
                            }
                        })}
                    }
                    .into_any()
                }
            })
        }}
        {move || note.get().map(|n| view! { <p class="hint small">{n}</p> })}

        <div class="actions">
            <button class="pill" class:on=move || auto.get() on:click=toggle_auto>
                {move || if auto.get() { "✓ Update automatically" } else { "Update automatically" }}
            </button>
        </div>
        <p class="hint small">
            {move || match status.get() {
                Some(s) if !s.allowed => {
                    "To update itself, Nextep needs Android's permission to install apps.".to_string()
                }
                Some(api::UpdateStatus { owner: Some(owner), .. }) => {
                    let owner = match owner.as_str() {
                        "dev.imranr.obtainium" => "Obtainium",
                        "com.android.vending" => "Google Play",
                        other => other,
                    };
                    format!(
                        "{owner} manages Nextep's updates on this phone, so Android asks before Nextep updates itself. Updates through {owner} still work."
                    )
                }
                Some(s) if !s.silent => {
                    "On this Android version, Android asks you to confirm each update.".to_string()
                }
                _ if auto.get() => {
                    "Each time Nextep opens it checks for a new version, downloads it, and installs it a few seconds after you leave. No Obtainium needed."
                        .to_string()
                }
                _ => "New versions show a banner; tap Update to install.".to_string(),
            }}
        </p>
        {move || {
            status.get().is_some_and(|s| !s.allowed).then(|| {
                view! {
                    <button class="pill" on:click=allow>"Allow installing updates"</button>
                }
            })
        }}
    }
}
