use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;

use crate::api::{self, EpisodeRow, SeasonRow, ShowDetails, ShowStatus};
use crate::app::{PagePreview, nav};
use crate::lists::ListPicker;

/// Full-screen show page over the tabs. Closed with the back button or
/// Android's back gesture.
#[component]
pub fn ShowPage(show_id: u64) -> impl IntoView {
    let nav = nav();
    let data = RwSignal::new(None::<ShowDetails>);
    let error = RwSignal::new(None::<String>);
    let open_season = RwSignal::new(None::<u32>);
    let preview = nav.show_preview(show_id);

    let show = move |d: ShowDetails| {
        // Start with the season of the next episode expanded.
        if open_season.get_untracked().is_none() {
            open_season.set(d.next_episode.as_ref().map(|e| e.season));
        }
        data.set(Some(d));
    };
    // The copy saved last time first, so a page opened before appears at
    // once, then fresh details from Trakt.
    let load = move || {
        spawn_local(async move {
            if data.with_untracked(Option::is_none)
                && let Ok(d) = fetch(show_id, true).await
            {
                show(d);
            }
            match fetch(show_id, false).await {
                Ok(d) => show(d),
                Err(e) => nav.report(e, error),
            }
        });
    };
    load();

    let on_mark = Callback::new(move |episode_id: u64| {
        spawn_local(async move {
            match api::mark_watched(episode_id).await {
                Ok(()) => {
                    nav.data_changed();
                    load();
                }
                Err(e) => nav.report(e, error),
            }
        });
    });

    let on_status = Callback::new(move |status: Option<ShowStatus>| {
        spawn_local(async move {
            match api::set_show_status(show_id, status).await {
                Ok(()) => {
                    data.update(|d| {
                        if let Some(d) = d {
                            d.status = status;
                        }
                    });
                    nav.data_changed();
                }
                Err(e) => nav.report(e, error),
            }
        });
    });

    view! {
        <div class="sheet">
            <button class="back" aria-label="Back" on:click=move |_| nav.close()>
                "←"
            </button>
            {move || error.get().map(|e| view! { <p class="warn pad">{e}</p> })}
            {move || match data.get() {
                None => view! { <PagePreview preview=preview.clone()/> }.into_any(),
                Some(d) => view! { <Details d on_mark on_status open_season/> }.into_any(),
            }}
        </div>
    }
}

async fn fetch(show_id: u64, cached: bool) -> Result<ShowDetails, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args {
        show_id: u64,
        region: String,
        cached: bool,
    }
    let args = Args {
        show_id,
        region: api::region(),
        cached,
    };
    api::call("show_details", args).await
}

#[component]
fn Details(
    d: ShowDetails,
    on_mark: Callback<u64>,
    on_status: Callback<Option<ShowStatus>>,
    open_season: RwSignal<Option<u32>>,
) -> impl IntoView {
    let show = d.show;
    let meta = [
        show.year.map(|y| y.to_string()),
        show.network.clone(),
        show.status.as_deref().map(capitalize),
        show.runtime.map(|r| format!("{r} min")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let rating = show
        .rating
        .filter(|r| *r > 0.0)
        .map(|r| format!("★ {r:.1}"));
    let percent = (d.completed * 100).checked_div(d.aired).unwrap_or(0);
    let hero = d.backdrop.or(d.poster);
    let status = d.status;
    // Labels only make sense once you've started a show; before that, offer
    // the watchlist instead.
    let started = d.completed > 0;
    let show_id = show.ids.trakt.unwrap_or_default();
    let status_hint = match status {
        Some(ShowStatus::OnHold) => {
            "On hold: hidden from Progress and Calendar until you watch it again."
        }
        Some(ShowStatus::Dropped) => {
            "Dropped: hidden from Progress and Calendar until you watch it again."
        }
        None => "",
    };

    let next = d.next_episode.map(|ep| {
        let aired = ep.first_aired.as_deref().is_some_and(has_aired);
        let when = ep.first_aired.as_deref().map(api::short_date);
        let id = ep.ids.trakt;
        view! {
            <div class="next">
                <div class="info">
                    <p class="meta">"Next episode"</p>
                    <p class="episode">
                        <span class="ep-code">{ep.code()}</span>
                        " "
                        {ep.title.clone().unwrap_or_default()}
                    </p>
                    {(!aired).then(|| view! { <p class="meta">{when.unwrap_or_else(|| "Date to be announced".into())}</p> })}
                </div>
                {aired
                    .then_some(id)
                    .flatten()
                    .map(|id| {
                        view! {
                            <button class="check" aria-label="Mark watched" on:click=move |_| on_mark.run(id)>
                                "✓"
                            </button>
                        }
                    })}
            </div>
        }
    });

    view! {
        <div class="hero">
            {hero.map(|src| view! { <img class="hero-img" src=src alt=""/> })}
            <div class="hero-fade"></div>
        </div>
        <div class="details">
            <h1 class="show-title">{show.display_title()}</h1>
            <p class="meta">{meta}</p>
            {rating.map(|r| view! { <p class="rating">{r}</p> })}

            <ListPicker show_id=show_id/>
            {if started {
                view! {
                    <div class="segmented">
                        {[(None, "Watching"), (Some(ShowStatus::OnHold), "On hold"), (Some(ShowStatus::Dropped), "Dropped")]
                            .into_iter()
                            .map(|(s, label)| {
                                let active = status == s;
                                view! {
                                    <button class:active=active on:click=move |_| on_status.run(s)>
                                        {label}
                                    </button>
                                }
                            })
                            .collect_view()}
                    </div>
                    <p class="hint small">{status_hint}</p>
                }
                    .into_any()
            } else {
                view! { <WatchlistButton show_id=show_id on=d.on_watchlist/> }.into_any()
            }}

            <p class="progress-line">{format!("{} of {} episodes watched", d.completed, d.aired)}</p>
            <div class="bar">
                <div class="fill" style=format!("width: {percent}%")></div>
            </div>
            {next}
            {show.overview.map(|o| view! { <p class="overview">{o}</p> })}
            {(!d.streaming.is_empty())
                .then(|| {
                    view! {
                        <h2 class="section">"Where to watch"</h2>
                        <div class="providers">
                            {d.streaming
                                .into_iter()
                                .map(|p| {
                                    view! {
                                        <span class="provider">
                                            {p.logo.map(|src| view! { <img src=src alt=""/> })}
                                            {p.name}
                                        </span>
                                    }
                                })
                                .collect_view()}
                        </div>
                        <p class="hint small">"Streaming data from JustWatch"</p>
                    }
                })}

            <h2 class="section">"Seasons"</h2>
            <ul class="list">
                {d.seasons
                    .into_iter()
                    .map(|season| view! { <SeasonView season on_mark open_season/> })
                    .collect_view()}
            </ul>
        </div>
    }
}

#[component]
fn SeasonView(
    season: SeasonRow,
    on_mark: Callback<u64>,
    open_season: RwSignal<Option<u32>>,
) -> impl IntoView {
    let number = season.number;
    let percent = (season.completed * 100)
        .checked_div(season.aired)
        .unwrap_or(0);
    let counts = format!("{}/{}", season.completed, season.aired);
    let episodes = season.episodes;
    let toggle = move |_| {
        open_season.update(|open| {
            *open = if *open == Some(number) {
                None
            } else {
                Some(number)
            }
        })
    };

    view! {
        <li class="season">
            <button class="season-head" on:click=toggle>
                <span class="season-name">{format!("Season {number}")}</span>
                <span class="meta">{counts}</span>
            </button>
            <div class="bar">
                <div class="fill" style=format!("width: {percent}%")></div>
            </div>
            {move || {
                (open_season.get() == Some(number))
                    .then(|| {
                        view! {
                            <ul class="episodes">
                                {episodes
                                    .clone()
                                    .into_iter()
                                    .map(|episode| view! { <EpisodeView episode on_mark/> })
                                    .collect_view()}
                            </ul>
                        }
                    })
            }}
        </li>
    }
}

#[component]
fn EpisodeView(episode: EpisodeRow, on_mark: Callback<u64>) -> impl IntoView {
    let aired = episode.first_aired.as_deref().is_some_and(has_aired);
    let marking = RwSignal::new(false);
    let right = match (episode.watched, aired, episode.trakt_id) {
        (true, _, _) => view! { <span class="watched">"✓"</span> }.into_any(),
        (false, true, Some(id)) => view! {
            <button
                class="check small"
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
        .into_any(),
        _ => {
            let when = episode
                .first_aired
                .as_deref()
                .map(api::short_date)
                .unwrap_or_else(|| "TBA".into());
            view! { <span class="meta">{when}</span> }.into_any()
        }
    };
    view! {
        <li class="ep" class:done=episode.watched>
            <span class="ep-num">{format!("E{:02}", episode.number)}</span>
            <span class="ep-title">{episode.title.unwrap_or_default()}</span>
            {right}
        </li>
    }
}

fn has_aired(iso: &str) -> bool {
    api::date(iso).get_time() <= js_sys::Date::now()
}

pub(crate) fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[component]
fn WatchlistButton(show_id: u64, on: bool) -> impl IntoView {
    let nav = nav();
    let on = RwSignal::new(on);
    let busy = RwSignal::new(false);
    let toggle = move |_| {
        let want = !on.get_untracked();
        busy.set(true);
        spawn_local(async move {
            if api::set_watchlist(show_id, want).await.is_ok() {
                on.set(want);
                nav.data_changed();
            }
            busy.set(false);
        });
    };
    view! {
        <button class="pill wide" class:on=move || on.get() disabled=move || busy.get() on:click=toggle>
            {move || if on.get() { "✓ On your watchlist" } else { "+ Add to watchlist" }}
        </button>
    }
}
