//! Profile page: the signed-in Trakt account, its totals, and the way to
//! Settings and signing out.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};

use crate::api;
use crate::app::nav;
use crate::library::format_count;

#[derive(Clone, Deserialize)]
struct Profile {
    username: String,
    name: Option<String>,
    avatar: Option<String>,
    vip: bool,
    joined_at: Option<String>,
    location: Option<String>,
    url: String,
    totals: Option<Totals>,
}

#[derive(Clone, Deserialize)]
struct Totals {
    shows: u64,
    episodes: u64,
    movies: u64,
    minutes: u64,
}

#[component]
pub fn ProfilePage() -> impl IntoView {
    let nav = nav();
    let data = RwSignal::new(None::<Profile>);
    let error = RwSignal::new(None::<String>);

    // The copy saved last time first, then fresh from Trakt.
    spawn_local(async move {
        #[derive(Serialize)]
        struct Args {
            cached: bool,
        }
        if let Ok(p) = api::call::<Profile>("profile", Args { cached: true }).await {
            data.set(Some(p));
        }
        match api::call::<Profile>("profile", Args { cached: false }).await {
            Ok(p) => data.set(Some(p)),
            Err(e) => nav.report(e, error),
        }
    });

    let sign_out = move |_| {
        spawn_local(async move {
            let _ = api::call::<()>("sign_out", ()).await;
            nav.sign_out();
        })
    };

    view! {
        <div class="sheet">
            <button class="back" aria-label="Back" on:click=move |_| nav.close()>
                "←"
            </button>
            <div class="details list-page">
                {move || error.get().map(|e| view! { <p class="warn">{e}</p> })}
                {move || match data.get() {
                    None => view! { <p class="hint">"Loading your profile…"</p> }.into_any(),
                    Some(p) => view! { <Account p/> }.into_any(),
                }}
                <div class="profile-actions">
                    <button class="pill wide" on:click=move |_| nav.open_settings()>
                        "Settings"
                    </button>
                    {move || {
                        data.get().map(|p| {
                            let url = p.url.clone();
                            view! {
                                <button
                                    class="pill wide"
                                    on:click=move |_| {
                                        let url = url.clone();
                                        spawn_local(async move { api::open_link(&url).await });
                                    }
                                >
                                    "Open profile on Trakt"
                                </button>
                            }
                        })
                    }}
                    <button class="ghost" on:click=sign_out>
                        "Sign out of Trakt"
                    </button>
                </div>
                <p class="version">{move || format!("Nextep v{}", nav.version.get())}</p>
            </div>
        </div>
    }
}

#[component]
fn Account(p: Profile) -> impl IntoView {
    let shown_name = p.name.clone().unwrap_or_else(|| p.username.clone());
    let initial = shown_name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let since = [
        p.joined_at
            .as_deref()
            .and_then(month_year)
            .map(|d| format!("Member since {d}")),
        p.location.clone(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");

    view! {
        <div class="profile-head">
            {match p.avatar {
                Some(src) => view! { <img class="avatar" src=src alt=""/> }.into_any(),
                None => view! { <div class="avatar blank">{initial}</div> }.into_any(),
            }}
            <div class="info">
                <h1 class="show-title">{shown_name}</h1>
                <p class="meta">
                    {format!("@{}", p.username)}
                    {p.vip.then(|| view! { <span class="badge">"VIP"</span> })}
                </p>
                {(!since.is_empty()).then(|| view! { <p class="meta">{since}</p> })}
            </div>
        </div>
        {match p.totals {
            Some(t) => view! { <TotalsGrid t/> }.into_any(),
            None => view! { <p class="hint small">"Trakt isn't sharing your totals right now."</p> }.into_any(),
        }}
    }
}

#[component]
fn TotalsGrid(t: Totals) -> impl IntoView {
    let hours = t.minutes / 60;
    let (time, time_label) = if hours >= 48 {
        (format_count(hours / 24), "days watched")
    } else {
        (format_count(hours), "hours watched")
    };
    view! {
        <div class="stats profile-stats">
            <div class="stat">
                <span class="stat-value">{format_count(t.shows)}</span>
                <span class="stat-label">"shows"</span>
            </div>
            <div class="stat">
                <span class="stat-value">{format_count(t.episodes)}</span>
                <span class="stat-label">"episodes"</span>
            </div>
            <div class="stat">
                <span class="stat-value">{format_count(t.movies)}</span>
                <span class="stat-label">"movies"</span>
            </div>
            <div class="stat">
                <span class="stat-value">{time}</span>
                <span class="stat-label">{time_label}</span>
            </div>
        </div>
    }
}

/// "Sep 2010" from an ISO date.
fn month_year(iso: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let year = iso.get(0..4)?;
    let month: usize = iso.get(5..7)?.parse().ok()?;
    Some(format!("{} {year}", MONTHS.get(month.checked_sub(1)?)?))
}
