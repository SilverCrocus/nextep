//! Settings page. For now: the AI provider and key for AI picks.

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

                <h2 class="section">"AI picks"</h2>
                <p class="hint small">
                    "Describe a mood in Discover and an AI picks from your Discover shows. "
                    "Works with DeepSeek or any OpenAI-compatible API. Your key stays on this phone; "
                    "each request sends your mood, titles you liked and dropped, and the candidate shows to the provider."
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
                        <option value="max">"Max (best picks, slower)"</option>
                        <option value="high">"High"</option>
                        <option value="low">"Low"</option>
                        <option value="none">"Off (fastest)"</option>
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

                <p class="version">{move || format!("Nextep v{}", nav.version.get())}</p>
            </div>
        </div>
    }
}
