use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"])]
    async fn invoke(cmd: &str, args: JsValue) -> JsValue;
}

#[derive(Clone, Deserialize)]
struct AppStatus {
    version: String,
    trakt_configured: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Progress,
    Calendar,
    Library,
}

impl Tab {
    const ALL: [Tab; 3] = [Tab::Progress, Tab::Calendar, Tab::Library];

    fn label(self) -> &'static str {
        match self {
            Tab::Progress => "Progress",
            Tab::Calendar => "Calendar",
            Tab::Library => "Library",
        }
    }
}

#[component]
pub fn App() -> impl IntoView {
    let (tab, set_tab) = signal(Tab::Progress);
    let (status, set_status) = signal(None::<AppStatus>);

    spawn_local(async move {
        let value = invoke("app_status", JsValue::NULL).await;
        set_status.set(serde_wasm_bindgen::from_value(value).ok());
    });

    view! {
        <main class="screen">
            <header class="top">
                <h1>{move || tab.get().label()}</h1>
            </header>

            <section class="content">
                <ConnectTrakt status=status/>
            </section>

            <nav class="tabs">
                {Tab::ALL
                    .into_iter()
                    .map(|t| {
                        view! {
                            <button
                                class:active=move || tab.get() == t
                                on:click=move |_| set_tab.set(t)
                            >
                                {t.label()}
                            </button>
                        }
                    })
                    .collect_view()}
            </nav>
        </main>
    }
}

/// Shown until the user signs in to Trakt.
#[component]
fn ConnectTrakt(status: ReadSignal<Option<AppStatus>>) -> impl IntoView {
    view! {
        <div class="empty">
            <p class="title">"Connect Trakt to see your shows"</p>
            <p class="hint">
                "Nextep reads what you watch from Trakt, including anything Stremio scrobbles."
            </p>
            {move || match status.get() {
                Some(s) if s.trakt_configured => {
                    view! { <button class="primary" disabled>"Sign in with Trakt (soon)"</button> }
                        .into_any()
                }
                Some(_) => {
                    view! {
                        <p class="warn">
                            "This build has no Trakt API keys. Add them to .env and rebuild."
                        </p>
                    }
                        .into_any()
                }
                None => view! { <p class="hint">"Loading…"</p> }.into_any(),
            }}
            <p class="version">
                {move || status.get().map(|s| format!("v{}", s.version)).unwrap_or_default()}
            </p>
        </div>
    }
}
