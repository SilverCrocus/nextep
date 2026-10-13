//! Movies: Progress (to watch / coming soon), Library (watched /
//! watchlist) and the movie page.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;

use crate::api::{self, MovieCard, MovieDetails, MoviesView};
use crate::app::{PagePreview, Poster, Preview, nav};
use crate::lists::ListPicker;
use crate::show::capitalize;

/// Loads the movie lists (saved copy first, then fresh) and reloads on
/// changes and pull-to-refresh.
fn load_movies() -> (RwSignal<Option<MoviesView>>, RwSignal<Option<String>>) {
    let nav = nav();
    let data = RwSignal::new(None::<MoviesView>);
    let error = RwSignal::new(None::<String>);
    let load = move |force: bool| {
        #[derive(Serialize)]
        struct Args {
            force: bool,
            cached: bool,
        }
        spawn_local(async move {
            if data.with_untracked(Option::is_none)
                && let Ok(v) = api::call::<MoviesView>(
                    "movies",
                    Args {
                        force,
                        cached: true,
                    },
                )
                .await
            {
                data.set(Some(v));
            }
            match api::call::<MoviesView>(
                "movies",
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
    (data, error)
}

/// Progress in Movies mode.
#[component]
pub fn MovieProgress() -> impl IntoView {
    let nav = nav();
    let (data, error) = load_movies();
    let on_watched = Callback::new(move |movie_id: u64| {
        // Drop it from "To watch" straight away.
        data.update(|d| {
            if let Some(d) = d {
                d.to_watch.retain(|m| m.movie_id != movie_id);
            }
        });
        spawn_local(async move {
            match api::mark_movie_watched(movie_id).await {
                Ok(()) => nav.data_changed(),
                Err(e) => nav.report(e, error),
            }
        });
    });

    view! {
        {move || error.get().map(|e| view! { <p class="warn">{e}</p> })}
        {move || match data.get() {
            None => view! { <p class="hint">"Loading your movies…"</p> }.into_any(),
            Some(v) if v.to_watch.is_empty() && v.coming_soon.is_empty() => view! {
                <p class="hint">"Nothing on your movie watchlist. Find movies with Search and add them."</p>
            }
            .into_any(),
            Some(v) => view! {
                {(!v.to_watch.is_empty())
                    .then(|| {
                        view! {
                            <h2 class="section">"To watch" <span class="count">{v.to_watch.len()}</span></h2>
                            <ul class="list">
                                {v.to_watch
                                    .into_iter()
                                    .map(|movie| view! { <MovieRow movie on_watched=Some(on_watched)/> })
                                    .collect_view()}
                            </ul>
                        }
                    })}
                {(!v.coming_soon.is_empty())
                    .then(|| {
                        view! {
                            <h2 class="section">"Coming soon" <span class="count">{v.coming_soon.len()}</span></h2>
                            <ul class="list">
                                {v.coming_soon
                                    .into_iter()
                                    .map(|movie| view! { <MovieRow movie on_watched=None/> })
                                    .collect_view()}
                            </ul>
                        }
                    })}
            }
            .into_any(),
        }}
    }
}

#[component]
fn MovieRow(movie: MovieCard, on_watched: Option<Callback<u64>>) -> impl IntoView {
    let nav = nav();
    let id = movie.movie_id;
    let preview = Preview::new(&movie.title, &movie.poster);
    view! {
        <li class="row">
            <div class="tap" on:click=move |_| nav.open_movie(id, preview.clone())>
                <Poster src=movie.poster class="thumb"/>
                <div class="info">
                    <p class="show">{movie.title}</p>
                    <p class="meta">{movie.note}</p>
                </div>
            </div>
            {on_watched.map(|cb| {
                view! {
                    <button class="check" aria-label="Mark watched" on:click=move |_| cb.run(id)>
                        "✓"
                    </button>
                }
            })}
        </li>
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Group {
    Watched,
    Watchlist,
}

/// Library in Movies mode.
#[component]
pub fn MovieLibrary() -> impl IntoView {
    let (data, error) = load_movies();
    let group = RwSignal::new(Group::Watched);
    view! {
        {move || error.get().map(|e| view! { <p class="warn">{e}</p> })}
        {move || match data.get() {
            None => view! { <p class="hint">"Loading…"</p> }.into_any(),
            Some(v) => {
                let counts = (v.watched.len(), v.watchlist.len());
                let chip = move |g: Group, label: &'static str, count: usize| {
                    view! {
                        <button class="chip" class:active=move || group.get() == g on:click=move |_| group.set(g)>
                            {label}
                            <span class="count">{count}</span>
                        </button>
                    }
                };
                view! {
                    <div class="chips">
                        {chip(Group::Watched, "Watched", counts.0)}
                        {chip(Group::Watchlist, "Watchlist", counts.1)}
                    </div>
                    {move || {
                        let cards = match group.get() {
                            Group::Watched => v.watched.clone(),
                            Group::Watchlist => v.watchlist.clone(),
                        };
                        if cards.is_empty() {
                            return view! { <p class="hint">"Nothing here."</p> }.into_any();
                        }
                        view! {
                            <div class="grid">
                                {cards.into_iter().map(|movie| view! { <MovieTile movie/> }).collect_view()}
                            </div>
                        }
                        .into_any()
                    }}
                }
                .into_any()
            }
        }}
    }
}

#[component]
fn MovieTile(movie: MovieCard) -> impl IntoView {
    let nav = nav();
    let id = movie.movie_id;
    let preview = Preview::new(&movie.title, &movie.poster);
    view! {
        <button class="card" on:click=move |_| nav.open_movie(id, preview.clone())>
            <Poster src=movie.poster/>
            <span class="card-title">{movie.title}</span>
            <span class="card-note">{movie.note}</span>
        </button>
    }
}

/// Full-screen movie page.
#[component]
pub fn MoviePage(movie_id: u64) -> impl IntoView {
    let nav = nav();
    let data = RwSignal::new(None::<MovieDetails>);
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let preview = nav.movie_preview(movie_id);

    // The copy saved last time first, then fresh details from Trakt.
    let load = move || {
        spawn_local(async move {
            if data.with_untracked(Option::is_none)
                && let Ok(d) = fetch_movie(movie_id, true).await
            {
                data.set(Some(d));
            }
            match fetch_movie(movie_id, false).await {
                Ok(d) => data.set(Some(d)),
                Err(e) => nav.report(e, error),
            }
        });
    };
    load();

    let watched = move |_| {
        busy.set(true);
        spawn_local(async move {
            match api::mark_movie_watched(movie_id).await {
                Ok(()) => {
                    nav.data_changed();
                    load();
                }
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        });
    };
    let toggle_watchlist = move |_| {
        let Some(on) = data.with_untracked(|d| d.as_ref().map(|d| !d.on_watchlist)) else {
            return;
        };
        busy.set(true);
        spawn_local(async move {
            match api::set_movie_watchlist(movie_id, on).await {
                Ok(()) => {
                    data.update(|d| {
                        if let Some(d) = d {
                            d.on_watchlist = on;
                        }
                    });
                    nav.data_changed();
                }
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        });
    };

    view! {
        <div class="sheet">
            <button class="back" aria-label="Back" on:click=move |_| nav.close()>
                "←"
            </button>
            {move || error.get().map(|e| view! { <p class="warn pad">{e}</p> })}
            {move || match data.get() {
                None => view! { <PagePreview preview=preview.clone()/> }.into_any(),
                Some(d) => {
                    let movie = d.movie;
                    let meta = [
                        movie.year.map(|y| y.to_string()),
                        movie.certification.clone().filter(|c| !c.is_empty()),
                        movie.runtime.filter(|r| *r > 0).map(|r| format!("{}h {:02}m", r / 60, r % 60)),
                        movie.genres.as_ref().map(|g| g.iter().take(3).map(|g| capitalize(g)).collect::<Vec<_>>().join(", ")),
                    ]
                    .into_iter()
                    .flatten()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ");
                    let rating = movie.rating.filter(|r| *r > 0.0).map(|r| format!("★ {r:.1}"));
                    let status = match (d.plays, d.last_watched_at.as_deref()) {
                        (0, _) if !d.released => match &movie.released {
                            Some(date) => format!("Out {}", api::short_date(&format!("{date}T12:00:00Z"))),
                            None => "Release date TBA".to_string(),
                        },
                        (0, _) => "Not watched yet".to_string(),
                        (1, Some(at)) => format!("Watched {}", api::short_date(at)),
                        (n, _) => format!("Watched {n} times"),
                    };
                    let hero = d.backdrop.or(d.poster);
                    view! {
                        <div class="hero">
                            {hero.map(|src| view! { <img class="hero-img" src=src alt=""/> })}
                            <div class="hero-fade"></div>
                        </div>
                        <div class="details">
                            <h1 class="show-title">{movie.title.clone().unwrap_or_default()}</h1>
                            <p class="meta">{meta}</p>
                            {rating.map(|r| view! { <p class="rating">{r}</p> })}
                            {movie.tagline.clone().filter(|t| !t.is_empty()).map(|t| view! { <p class="hint">{t}</p> })}
                            <p class="progress-line">{status}</p>
                            <div class="actions">
                                {d.released
                                    .then(|| {
                                        view! {
                                            <button class="pill on" on:click=watched disabled=move || busy.get()>
                                                {if d.plays > 0 { "✓ Watched again" } else { "✓ Mark watched" }}
                                            </button>
                                        }
                                    })}
                                <button class="pill" class:on=d.on_watchlist on:click=toggle_watchlist disabled=move || busy.get()>
                                    {if d.on_watchlist { "✓ On your watchlist" } else { "+ Watchlist" }}
                                </button>
                            </div>
                            <ListPicker id=movie_id movie=true/>
                            {movie.overview.clone().map(|o| view! { <p class="overview">{o}</p> })}
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
                            {(!d.directors.is_empty())
                                .then(|| view! { <p class="meta credits">{format!("Directed by {}", d.directors.join(", "))}</p> })}
                            {(!d.cast.is_empty())
                                .then(|| view! { <p class="meta credits">{format!("Starring {}", d.cast.join(", "))}</p> })}
                        </div>
                    }
                    .into_any()
                }
            }}
        </div>
    }
}

async fn fetch_movie(movie_id: u64, cached: bool) -> Result<MovieDetails, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args {
        movie_id: u64,
        region: String,
        cached: bool,
    }
    let args = Args {
        movie_id,
        region: api::region(),
        cached,
    };
    api::call("movie_details", args).await
}
