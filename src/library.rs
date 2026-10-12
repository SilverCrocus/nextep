use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;

use crate::api::{self, Card, LibraryView, Stats};
use crate::app::{Poster, nav};
use crate::lists::ListsPanel;

#[derive(Clone, Copy, PartialEq)]
enum Group {
    Watching,
    UpToDate,
    Finished,
    OnHold,
    Dropped,
    Watchlist,
    /// Trakt lists; shown by [`ListsPanel`] rather than as a poster grid.
    Lists,
}

impl Group {
    const ALL: [Group; 7] = [
        Group::Watching,
        Group::UpToDate,
        Group::OnHold,
        Group::Dropped,
        Group::Finished,
        Group::Watchlist,
        Group::Lists,
    ];

    fn label(self) -> &'static str {
        match self {
            Group::Watching => "Watching",
            Group::UpToDate => "Up to date",
            Group::Finished => "Finished",
            Group::OnHold => "On hold",
            Group::Dropped => "Dropped",
            Group::Watchlist => "Watchlist",
            Group::Lists => "Lists",
        }
    }

    fn cards(self, view: &LibraryView) -> &[Card] {
        match self {
            Group::Watching => &view.watching,
            Group::UpToDate => &view.up_to_date,
            Group::Finished => &view.finished,
            Group::OnHold => &view.on_hold,
            Group::Dropped => &view.dropped,
            Group::Watchlist => &view.watchlist,
            Group::Lists => &[],
        }
    }
}

#[component]
pub fn Library() -> impl IntoView {
    let nav = nav();
    let data = RwSignal::new(None::<LibraryView>);
    let error = RwSignal::new(None::<String>);
    let group = RwSignal::new(Group::Watching);

    let load = move |force: bool| {
        #[derive(Serialize)]
        struct Args {
            force: bool,
            cached: bool,
        }
        error.set(None);
        spawn_local(async move {
            // Last saved library first, then fresh data.
            if data.get_untracked().is_none()
                && let Ok(v) = api::call::<LibraryView>(
                    "library",
                    Args {
                        force,
                        cached: true,
                    },
                )
                .await
            {
                data.set(Some(v));
            }
            match api::call::<LibraryView>(
                "library",
                Args {
                    force,
                    cached: false,
                },
            )
            .await
            {
                Ok(v) => data.set(Some(v)),
                Err(e) => nav.report(e, error),
            }
        });
    };
    Effect::new(move |_| {
        nav.changed.track();
        load(false);
    });
    Effect::new(move |first_run: Option<()>| {
        nav.refresh.track();
        if first_run.is_some() {
            load(true);
        }
    });

    let sign_out = move |_| {
        spawn_local(async move {
            let _ = api::call::<()>("sign_out", ()).await;
            nav.sign_out();
        })
    };

    view! {
        {move || error.get().map(|e| view! { <p class="warn">{e}</p> })}
        {move || match data.get() {
            None => view! { <p class="hint">"Loading…"</p> }.into_any(),
            Some(v) => {
                let chips = Group::ALL
                    .into_iter()
                    .map(|g| {
                        let count = (g != Group::Lists).then(|| g.cards(&v).len());
                        view! {
                            <button class="chip" class:active=move || group.get() == g on:click=move |_| group.set(g)>
                                {g.label()}
                                {count.map(|n| view! { <span class="count">{n}</span> })}
                            </button>
                        }
                    })
                    .collect_view();
                let v2 = v.clone();
                view! {
                    <div class="chips">{chips}</div>
                    {move || {
                        if group.get() == Group::Lists {
                            return view! { <ListsPanel/> }.into_any();
                        }
                        let cards = group.get().cards(&v2).to_vec();
                        if cards.is_empty() {
                            return view! { <p class="hint">"Nothing here."</p> }.into_any();
                        }
                        view! {
                            <div class="grid">
                                {cards.into_iter().map(|card| view! { <CardView card/> }).collect_view()}
                            </div>
                        }
                        .into_any()
                    }}
                }
                .into_any()
            }
        }}
        <StatsStrip/>
        <div class="footer">
            <button class="ghost" on:click=move |_| nav.open_settings()>"Settings"</button>
            <button class="ghost" on:click=sign_out>"Sign out of Trakt"</button>
            <p class="version">{move || format!("Nextep v{}", nav.version.get())}</p>
        </div>
    }
}

#[component]
fn CardView(card: Card) -> impl IntoView {
    let nav = nav();
    let show_id = card.show_id;
    view! {
        <button class="card" on:click=move |_| nav.open_show(show_id)>
            <Poster src=card.poster/>
            <span class="card-title">{card.title}</span>
            <span class="card-note">{card.note}</span>
        </button>
    }
}

/// Totals from Trakt: episodes, time spent, shows and favourite genres.
#[component]
fn StatsStrip() -> impl IntoView {
    #[derive(Serialize)]
    struct Args {
        cached: bool,
    }
    let nav = nav();
    let stats = RwSignal::new(None::<Stats>);
    Effect::new(move |_| {
        nav.changed.track();
        nav.refresh.track();
        spawn_local(async move {
            if stats.with_untracked(Option::is_none)
                && let Ok(s) = api::call::<Stats>("stats", Args { cached: true }).await
            {
                stats.set(Some(s));
            }
            if let Ok(s) = api::call::<Stats>("stats", Args { cached: false }).await {
                stats.set(Some(s));
            }
        });
    });

    move || {
        stats.get().map(|s| {
            let days = s.minutes as f64 / 60.0 / 24.0;
            let genres = s
                .top_genres
                .iter()
                .map(|g| capitalize(g))
                .collect::<Vec<_>>()
                .join(", ");
            view! {
                <div class="stats">
                    <div class="stat">
                        <span class="stat-value">{format_count(s.episodes)}</span>
                        <span class="stat-label">"episodes"</span>
                    </div>
                    <div class="stat">
                        <span class="stat-value">{format!("{days:.0}")}</span>
                        <span class="stat-label">"days watched"</span>
                    </div>
                    <div class="stat">
                        <span class="stat-value">{format_count(s.shows)}</span>
                        <span class="stat-label">"shows"</span>
                    </div>
                </div>
                {(!genres.is_empty()).then(|| view! { <p class="hint small center-text">{format!("Mostly {genres}")}</p> })}
            }
        })
    }
}

/// 4531 → "4,531".
fn format_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}
