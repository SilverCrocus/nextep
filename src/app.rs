use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;

use crate::api::{self, DeviceCode, NOT_SIGNED_IN, Status};
use crate::calendar::Calendar;
use crate::discover::Discover;
use crate::library::Library;
use crate::lists::ListPage;
use crate::progress::Progress;
use crate::pull::PullToRefresh;
use crate::search::SearchPage;
use crate::show::ShowPage;

/// Full-screen pages shown over the tabs, newest last.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Overlay {
    Search,
    Show(u64),
    List(u64),
}

/// App-wide navigation and shared state, provided as context.
#[derive(Clone, Copy)]
pub struct Nav {
    overlays: RwSignal<Vec<Overlay>>,
    /// Bumped when data changes (episode marked, label set) so lists reload.
    pub changed: RwSignal<u32>,
    /// Bumped by pull-to-refresh: lists reload from Trakt, skipping caches.
    pub refresh: RwSignal<u32>,
    pub sort: RwSignal<String>,
    pub version: RwSignal<String>,
    on_signed_out: Callback<()>,
}

impl Nav {
    pub fn open_show(&self, show_id: u64) {
        self.push(Overlay::Show(show_id));
    }

    pub fn open_search(&self) {
        self.push(Overlay::Search);
    }

    pub fn open_list(&self, list_id: u64) {
        self.push(Overlay::List(list_id));
    }

    /// Each overlay gets a history entry so Android's back gesture closes it.
    fn push(&self, overlay: Overlay) {
        api::push_history();
        self.overlays.update(|o| o.push(overlay));
    }

    /// Closes the top overlay. Goes through history so the back gesture and
    /// the on-screen back button agree.
    pub fn close(&self) {
        api::history_back();
    }

    pub fn data_changed(&self) {
        self.changed.update(|n| *n += 1);
    }

    pub fn refresh(&self) {
        self.refresh.update(|n| *n += 1);
    }

    /// Shows an error from a backend call, or the sign-in screen if the
    /// session ended.
    pub fn report(&self, e: String, error: RwSignal<Option<String>>) {
        if e == NOT_SIGNED_IN {
            self.on_signed_out.run(());
        } else {
            error.set(Some(e));
        }
    }

    pub fn sign_out(&self) {
        self.on_signed_out.run(());
    }
}

pub fn nav() -> Nav {
    expect_context::<Nav>()
}

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
        match api::call::<Status>("app_status", ()).await {
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

/// Trakt device flow: show a code, the user approves it on trakt in the
/// browser, and we poll until they have.
#[component]
fn SignIn(on_signed_in: Callback<()>) -> impl IntoView {
    let code = RwSignal::new(None::<DeviceCode>);
    let message = RwSignal::new(None::<String>);

    let start = move |_| {
        message.set(None);
        spawn_local(async move {
            let dc = match api::call::<DeviceCode>("sign_in_start", ()).await {
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
            let outcome = loop {
                TimeoutFuture::new(1000).await;
                let now = js_sys::Date::now();
                if now - started >= expires_ms {
                    break "The code expired. Try again.".to_string();
                }
                // Android blocks network access for apps in the background, so
                // only poll while visible: on schedule, or right away when the
                // user comes back from approving in the browser.
                let hidden = api::page_hidden();
                let came_back = was_hidden && !hidden;
                was_hidden = hidden;
                if hidden || (now - last_poll < interval_ms && !came_back) {
                    continue;
                }
                last_poll = now;
                let args = Args {
                    device_code: &dc.device_code,
                };
                match api::call::<String>("sign_in_poll", args).await.as_deref() {
                    Ok("signed_in") => return on_signed_in.run(()),
                    Ok("pending") => message.set(None),
                    Ok("slow_down") => interval_ms += 5000.0,
                    Ok("denied") => break "Sign-in was declined on Trakt.".into(),
                    Ok(_) => break "The code expired. Try again.".into(),
                    // No network for a moment (e.g. just back from the
                    // background). Keep trying until the code expires.
                    Err(_) => message.set(Some("Can't reach Trakt, retrying…".into())),
                }
            };
            code.set(None);
            message.set(Some(outcome));
        });
    };

    view! {
        <main class="screen signin">
            <img class="brand-logo" src="logo.png" alt="" width="128" height="128"/>
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
                                api::copy_to_clipboard(&user_code);
                                let url = url.clone();
                                spawn_local(async move { api::open_link(&url).await });
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
    Discover,
    Library,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Progress, Tab::Calendar, Tab::Discover, Tab::Library];

    fn label(self) -> &'static str {
        match self {
            Tab::Progress => "Progress",
            Tab::Calendar => "Calendar",
            Tab::Discover => "Discover",
            Tab::Library => "Library",
        }
    }
}

#[component]
fn Main(on_signed_out: Callback<()>, version: RwSignal<String>) -> impl IntoView {
    let tab = RwSignal::new(Tab::Progress);
    let overlays = RwSignal::new(Vec::<Overlay>::new());
    let sort = api::load_pref("sort").unwrap_or_else(|| "recently_watched".into());
    let nav = Nav {
        overlays,
        changed: RwSignal::new(0),
        refresh: RwSignal::new(0),
        sort: RwSignal::new(sort),
        version,
        on_signed_out,
    };
    provide_context(nav);
    // Coming back to the app (e.g. from Stremio) refreshes the current tab.
    let _ = window_event_listener_untyped("visibilitychange", move |_| {
        if !api::page_hidden() {
            nav.data_changed();
        }
    });
    // Android's back gesture (and the on-screen back buttons) pop the history
    // entry pushed when an overlay opened.
    let _ = window_event_listener_untyped("popstate", move |_| {
        overlays.update(|o| {
            o.pop();
        })
    });

    // Once per launch: is there a newer release on GitHub?
    let update = RwSignal::new(None::<api::Release>);
    spawn_local(async move {
        if let Ok(Some(release)) = api::call::<Option<api::Release>>("check_update", ()).await {
            update.set(Some(release));
        }
    });
    let update_banner = move || {
        update.get().map(|release| {
            let url = release.apk_url.unwrap_or(release.page_url);
            view! {
                <div class="update">
                    <span>{format!("Nextep {} is available", release.version)}</span>
                    <button
                        class="pill"
                        on:click=move |_| {
                            let url = url.clone();
                            spawn_local(async move { api::open_link(&url).await });
                        }
                    >
                        "Update"
                    </button>
                    <button class="icon" aria-label="Dismiss" on:click=move |_| update.set(None)>
                        "✕"
                    </button>
                </div>
            }
        })
    };

    // Keyed by position and page so pages underneath keep their state (e.g.
    // search results) while a show is opened on top.
    let overlay_stack = move || overlays.get().into_iter().enumerate().collect::<Vec<_>>();
    let render_overlay = |(_, overlay): (usize, Overlay)| match overlay {
        Overlay::Search => view! { <SearchPage/> }.into_any(),
        Overlay::Show(show_id) => view! { <ShowPage show_id/> }.into_any(),
        Overlay::List(list_id) => view! { <ListPage list_id/> }.into_any(),
    };

    view! {
        <main class="screen">
            <header class="top">
                <h1>{move || tab.get().label()}</h1>
                <button class="icon" aria-label="Search" on:click=move |_| nav.open_search()>
                    <SearchIcon/>
                </button>
            </header>
            {update_banner}
            <section class="content">
                <PullToRefresh on_refresh=Callback::new(move |_| nav.refresh())>
                    {move || match tab.get() {
                        Tab::Progress => view! { <Progress/> }.into_any(),
                        Tab::Calendar => view! { <Calendar/> }.into_any(),
                        Tab::Discover => view! { <Discover/> }.into_any(),
                        Tab::Library => view! { <Library/> }.into_any(),
                    }}
                </PullToRefresh>
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
            <For each=overlay_stack key=|entry| *entry children=render_overlay/>
        </main>
    }
}

#[component]
pub fn SearchIcon() -> impl IntoView {
    view! {
        <svg
            viewBox="0 0 24 24"
            width="22"
            height="22"
            fill="none"
            stroke="currentColor"
            stroke-width="2.2"
            stroke-linecap="round"
        >
            <circle cx="11" cy="11" r="7"></circle>
            <path d="m20 20-3.6-3.6"></path>
        </svg>
    }
}

/// Poster thumbnail, or a blank tile while there is none.
#[component]
pub fn Poster(src: Option<String>, #[prop(optional)] class: &'static str) -> impl IntoView {
    let class = if class.is_empty() { "poster" } else { class };
    match src {
        Some(src) => view! { <img class=class src=src loading="lazy" alt=""/> }.into_any(),
        None => view! { <div class=format!("{class} blank")></div> }.into_any(),
    }
}
