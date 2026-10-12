use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], catch)]
    async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;
}

/// Error string the backend returns when the user has to sign in (again).
const NOT_SIGNED_IN: &str = "not_signed_in";

/// Calls a Tauri command. Tauri expects camelCase argument names.
async fn call<T: DeserializeOwned>(cmd: &str, args: impl Serialize) -> Result<T, String> {
    let args = serde_wasm_bindgen::to_value(&args).map_err(|e| e.to_string())?;
    let value = invoke(cmd, args).await.map_err(|e| {
        e.as_string()
            .unwrap_or_else(|| "Something went wrong".into())
    })?;
    serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())
}

async fn open_trakt(url: &str) {
    #[derive(Serialize)]
    struct Args<'a> {
        url: &'a str,
    }
    let _ = call::<()>("open_trakt", Args { url }).await;
}

/// Best effort: the code is also shown on screen if the clipboard is blocked.
fn copy_to_clipboard(text: &str) {
    let get = |target: &JsValue, key: &str| js_sys::Reflect::get(target, &key.into()).ok();
    let clipboard = get(&js_sys::global(), "navigator").and_then(|nav| get(&nav, "clipboard"));
    if let Some(clipboard) = clipboard
        && let Some(write) = get(&clipboard, "writeText")
    {
        let _ = js_sys::Function::from(write).call1(&clipboard, &text.into());
    }
}

/// True while the app is in the background (`document.hidden`).
fn page_hidden() -> bool {
    js_sys::Reflect::get(&js_sys::global(), &"document".into())
        .and_then(|doc| js_sys::Reflect::get(&doc, &"hidden".into()))
        .ok()
        .and_then(|hidden| hidden.as_bool())
        .unwrap_or(false)
}

// ---- data from the backend (only the fields the UI uses) -------------------

#[derive(Clone, Deserialize)]
struct Status {
    version: String,
    trakt_configured: bool,
    signed_in: bool,
}

#[derive(Clone, Deserialize)]
struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_url: String,
    expires_in: u32,
    interval: u32,
}

#[derive(Clone, Deserialize)]
struct Ids {
    trakt: Option<u64>,
}

#[derive(Clone, Deserialize)]
struct Show {
    title: Option<String>,
}

#[derive(Clone, Deserialize)]
struct Episode {
    season: u32,
    number: u32,
    title: Option<String>,
    ids: Ids,
    first_aired: Option<String>,
}

#[derive(Clone, Deserialize)]
struct Item {
    show: Show,
    aired: u32,
    completed: u32,
    next_episode: Option<Episode>,
}

#[derive(Clone, Deserialize)]
struct ProgressView {
    new_episodes: Vec<Item>,
    watching: Vec<Item>,
    upcoming: Vec<Item>,
}

// ---- app shell -------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Loading,
    Unconfigured,
    SignIn,
    Main,
}

#[component]
pub fn App() -> impl IntoView {
    let screen = RwSignal::new(Screen::Loading);
    let version = RwSignal::new(String::new());

    spawn_local(async move {
        match call::<Status>("app_status", ()).await {
            Ok(s) => {
                version.set(s.version);
                screen.set(match (s.trakt_configured, s.signed_in) {
                    (false, _) => Screen::Unconfigured,
                    (true, true) => Screen::Main,
                    (true, false) => Screen::SignIn,
                });
            }
            Err(_) => screen.set(Screen::Unconfigured),
        }
    });

    let on_signed_in = Callback::new(move |_| screen.set(Screen::Main));
    let on_signed_out = Callback::new(move |_| screen.set(Screen::SignIn));

    move || {
        match screen.get() {
        Screen::Loading => view! { <p class="hint center">"Loading…"</p> }.into_any(),
        Screen::Unconfigured => view! {
            <p class="warn center">"This build has no Trakt Client ID. Add it to .env and rebuild."</p>
        }
        .into_any(),
        Screen::SignIn => view! { <SignIn on_signed_in/> }.into_any(),
        Screen::Main => view! { <Main on_signed_out version/> }.into_any(),
    }
    }
}

// ---- sign in ---------------------------------------------------------------

/// Trakt device flow: show a code, open trakt in the browser, poll until the
/// user approves.
#[component]
fn SignIn(on_signed_in: Callback<()>) -> impl IntoView {
    let code = RwSignal::new(None::<DeviceCode>);
    let message = RwSignal::new(None::<String>);

    let start = move |_| {
        message.set(None);
        spawn_local(async move {
            let dc = match call::<DeviceCode>("sign_in_start", ()).await {
                Ok(dc) => dc,
                Err(e) => return message.set(Some(e)),
            };
            // Show the code first; the user opens Trakt from the button so the
            // browser doesn't cover the code before they've seen it.
            code.set(Some(dc.clone()));

            #[derive(Serialize)]
            #[serde(rename_all = "camelCase")]
            struct Args<'a> {
                device_code: &'a str,
            }
            // Wall-clock time, because Android pauses timers while the app is
            // in the background.
            let started = js_sys::Date::now();
            let expires_ms = f64::from(dc.expires_in) * 1000.0;
            let mut interval_ms = f64::from(dc.interval.max(1)) * 1000.0;
            let mut last_poll = started;
            let mut was_hidden = false;
            let mut failures = 0;
            let outcome = loop {
                TimeoutFuture::new(1000).await;
                let now = js_sys::Date::now();
                if now - started >= expires_ms {
                    break "The code expired. Try again.".to_string();
                }
                // Poll on schedule, or right away when the user comes back
                // from approving in the browser.
                let hidden = page_hidden();
                let came_back = was_hidden && !hidden;
                was_hidden = hidden;
                if now - last_poll < interval_ms && !came_back {
                    continue;
                }
                last_poll = now;
                let args = Args {
                    device_code: &dc.device_code,
                };
                match call::<String>("sign_in_poll", args).await.as_deref() {
                    Ok("signed_in") => return on_signed_in.run(()),
                    Ok("pending") => failures = 0,
                    Ok("slow_down") => interval_ms += 5000.0,
                    Ok("denied") => break "Sign-in was declined on Trakt.".into(),
                    Ok(_) => break "The code expired. Try again.".into(),
                    // Usually a connection dropped while Android had the app
                    // frozen in the background; the next poll will succeed.
                    Err(e) => {
                        failures += 1;
                        if failures >= 5 {
                            break e.clone();
                        }
                    }
                }
            };
            code.set(None);
            message.set(Some(outcome));
        });
    };

    view! {
        <main class="screen signin">
            <h1 class="brand">"Nextep"</h1>
            {move || match code.get() {
                None => view! {
                    <p class="hint">
                        "Sign in with Trakt to see what to watch next. Anything Stremio scrobbles to Trakt shows up here too."
                    </p>
                    <button class="primary" on:click=start>"Sign in with Trakt"</button>
                }
                .into_any(),
                Some(dc) => {
                    let url = dc.verification_url.clone();
                    let user_code = dc.user_code.clone();
                    view! {
                        <p class="hint">"Your code:"</p>
                        <p class="user-code">{dc.user_code}</p>
                        <button
                            class="primary"
                            on:click=move |_| {
                                // Copy inside the tap so the clipboard allows it.
                                copy_to_clipboard(&user_code);
                                let url = url.clone();
                                spawn_local(async move { open_trakt(&url).await });
                            }
                        >
                            "Copy code & open Trakt"
                        </button>
                        <p class="hint small">
                            "Paste the code on Trakt and approve, then come back here. "
                            "Nextep continues on its own."
                        </p>
                    }
                    .into_any()
                }
            }}
            {move || message.get().map(|m| view! { <p class="warn">{m}</p> })}
        </main>
    }
}

// ---- main screen -----------------------------------------------------------

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
fn Main(on_signed_out: Callback<()>, version: RwSignal<String>) -> impl IntoView {
    let tab = RwSignal::new(Tab::Progress);

    view! {
        <main class="screen">
            <header class="top">
                <h1>{move || tab.get().label()}</h1>
            </header>
            <section class="content">
                {move || match tab.get() {
                    Tab::Progress => view! { <Progress on_signed_out/> }.into_any(),
                    Tab::Calendar => view! { <p class="hint">"Calendar is coming soon."</p> }.into_any(),
                    Tab::Library => view! { <Library on_signed_out version/> }.into_any(),
                }}
            </section>
            <nav class="tabs">
                {Tab::ALL
                    .into_iter()
                    .map(|t| {
                        view! {
                            <button class:active=move || tab.get() == t on:click=move |_| tab.set(t)>
                                {t.label()}
                            </button>
                        }
                    })
                    .collect_view()}
            </nav>
        </main>
    }
}

#[component]
fn Library(on_signed_out: Callback<()>, version: RwSignal<String>) -> impl IntoView {
    let sign_out = move |_| {
        spawn_local(async move {
            let _ = call::<()>("sign_out", ()).await;
            on_signed_out.run(());
        })
    };
    view! {
        <p class="hint">"My Shows and Watchlist are coming soon."</p>
        <button class="ghost" on:click=sign_out>"Sign out of Trakt"</button>
        <p class="version">{move || format!("Nextep v{}", version.get())}</p>
    }
}

// ---- progress --------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    New,
    Watching,
    Upcoming,
}

#[component]
fn Progress(on_signed_out: Callback<()>) -> impl IntoView {
    let data = RwSignal::new(None::<ProgressView>);
    let error = RwSignal::new(None::<String>);
    let loading = RwSignal::new(false);
    let sort = RwSignal::new("recently_watched".to_string());

    let load = move || {
        #[derive(Serialize)]
        struct Args {
            sort: String,
        }
        loading.set(true);
        error.set(None);
        spawn_local(async move {
            let result = call::<ProgressView>(
                "progress",
                Args {
                    sort: sort.get_untracked(),
                },
            )
            .await;
            loading.set(false);
            match result {
                Ok(v) => data.set(Some(v)),
                Err(e) if e == NOT_SIGNED_IN => on_signed_out.run(()),
                Err(e) => error.set(Some(e)),
            }
        });
    };
    load();

    let on_mark = Callback::new(move |episode_id: u64| {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Args {
            episode_id: u64,
        }
        spawn_local(async move {
            match call::<()>("mark_watched", Args { episode_id }).await {
                Ok(()) => load(),
                Err(e) if e == NOT_SIGNED_IN => on_signed_out.run(()),
                Err(e) => error.set(Some(e)),
            }
        });
    });

    view! {
        <div class="toolbar">
            <select on:change=move |ev| {
                sort.set(event_target_value(&ev));
                load();
            }>
                <option value="recently_watched">"Recently watched"</option>
                <option value="title">"Title"</option>
                <option value="episodes_left">"Fewest left"</option>
                <option value="newest_episode">"Newest episode"</option>
            </select>
            <button class="ghost" on:click=move |_| load() disabled=move || loading.get()>
                {move || if loading.get() { "Refreshing…" } else { "Refresh" }}
            </button>
        </div>
        {move || error.get().map(|e| view! { <p class="warn">{e}</p> })}
        {move || match data.get() {
            None => view! {
                <p class="hint">
                    "Loading your shows… The first load checks every show, so it can take a little while."
                </p>
            }
            .into_any(),
            Some(v) => view! {
                <Section title="New episodes" kind=Kind::New items=v.new_episodes on_mark/>
                <Section title="Watching" kind=Kind::Watching items=v.watching on_mark/>
                <Section title="Upcoming" kind=Kind::Upcoming items=v.upcoming on_mark/>
            }
            .into_any(),
        }}
    }
}

#[component]
fn Section(
    title: &'static str,
    kind: Kind,
    items: Vec<Item>,
    on_mark: Callback<u64>,
) -> impl IntoView {
    if items.is_empty() {
        return ().into_any();
    }
    view! {
        <h2 class="section">{title} <span class="count">{items.len()}</span></h2>
        <ul class="list">
            {items.into_iter().map(|item| view! { <Row item kind on_mark/> }).collect_view()}
        </ul>
    }
    .into_any()
}

#[component]
fn Row(item: Item, kind: Kind, on_mark: Callback<u64>) -> impl IntoView {
    let ep = item.next_episode;
    let code = ep
        .as_ref()
        .map(|e| format!("S{:02}E{:02}", e.season, e.number))
        .unwrap_or_default();
    let ep_title = ep
        .as_ref()
        .and_then(|e| e.title.clone())
        .unwrap_or_default();
    let episode_id = ep.as_ref().and_then(|e| e.ids.trakt);
    let date = ep
        .as_ref()
        .and_then(|e| e.first_aired.as_deref())
        .map(short_date);

    let left = item.aired.saturating_sub(item.completed);
    let meta = match kind {
        Kind::New => format!("{left} left · aired {}", date.unwrap_or_default()),
        Kind::Watching => format!("{left} left"),
        Kind::Upcoming => date.unwrap_or_else(|| "Date to be announced".into()),
    };
    let percent = (item.completed * 100).checked_div(item.aired).unwrap_or(0);
    let marking = RwSignal::new(false);

    view! {
        <li class="row">
            <div class="info">
                <p class="show">{item.show.title.unwrap_or_else(|| "Untitled".into())}</p>
                <p class="episode">
                    <span class="ep-code">{code}</span>
                    " "
                    {ep_title}
                </p>
                <p class="meta">{meta}</p>
                <div class="bar">
                    <div class="fill" style=format!("width: {percent}%")></div>
                </div>
            </div>
            {(kind != Kind::Upcoming)
                .then_some(episode_id)
                .flatten()
                .map(|id| {
                    view! {
                        <button
                            class="check"
                            aria-label="Mark watched"
                            disabled=move || marking.get()
                            on:click=move |_| {
                                marking.set(true);
                                on_mark.run(id);
                            }
                        >
                            "✓"
                        </button>
                    }
                })}
        </li>
    }
}

/// "Fri 16 Oct" in the device's language.
fn short_date(iso: &str) -> String {
    let set = |obj: &js_sys::Object, key: &str, value: &str| {
        let _ = js_sys::Reflect::set(obj, &key.into(), &value.into());
    };
    let options = js_sys::Object::new();
    set(&options, "weekday", "short");
    set(&options, "day", "numeric");
    set(&options, "month", "short");
    let language = js_sys::Reflect::get(&js_sys::global(), &"navigator".into())
        .and_then(|nav| js_sys::Reflect::get(&nav, &"language".into()))
        .ok()
        .and_then(|lang| lang.as_string())
        .unwrap_or_else(|| "en".into());
    js_sys::Date::new(&iso.into())
        .to_locale_date_string(&language, &options)
        .into()
}
