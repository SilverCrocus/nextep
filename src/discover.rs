use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;

use crate::api::{self, AiPick, DiscoverItem, DiscoverView};
use crate::app::{Media, Poster, Preview, nav};
use crate::settings::load_ai_settings;

/// Shows (or movies) you haven't watched, in shelves: picked by the
/// on-device taste model, related to what you watched last, Trakt's picks,
/// and trending.
#[component]
pub fn Discover() -> impl IntoView {
    let nav = nav();
    let data = RwSignal::new(None::<DiscoverView>);
    let error = RwSignal::new(None::<String>);

    let load = move |force: bool| {
        #[derive(Serialize)]
        struct Args {
            force: bool,
            cached: bool,
            movies: bool,
        }
        let media = nav.media.get_untracked();
        let movies = media == Media::Movies;
        // A slow answer for shows mustn't land after switching to movies.
        let current = move || nav.media.get_untracked() == media;
        spawn_local(async move {
            if data.with_untracked(Option::is_none)
                && let Ok(v) = api::call::<DiscoverView>(
                    "discover",
                    Args {
                        force,
                        cached: true,
                        movies,
                    },
                )
                .await
                && current()
            {
                data.set(Some(v));
            }
            let result = api::call::<DiscoverView>(
                "discover",
                Args {
                    force,
                    cached: false,
                    movies,
                },
            )
            .await;
            if !current() {
                return;
            }
            match result {
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
    Effect::new(move |previous: Option<Media>| {
        let media = nav.media.get();
        if previous.is_some_and(|p| p != media) {
            data.set(None);
            error.set(None);
            load(false);
        }
        media
    });

    view! {
        <AskAi/>
        {move || error.get().map(|e| view! { <p class="warn">{e}</p> })}
        {move || match data.get() {
            None => {
                let what = match nav.media.get_untracked() {
                    Media::Shows => "Finding shows for you…",
                    Media::Movies => "Finding movies for you…",
                };
                view! { <p class="hint">{what}</p> }.into_any()
            }
            Some(v) => view! {
                {(v.learned_from > 0)
                    .then(|| {
                        view! {
                            <p class="hint small">
                                {format!(
                                    "Match % comes from a model of your taste, learned on this phone from {} shows you finished or dropped.",
                                    v.learned_from,
                                )}
                            </p>
                        }
                    })}
                {v.rows
                    .into_iter()
                    .map(|row| {
                        view! {
                            <h2 class="section">{row.title}</h2>
                            <div class="shelf">
                                {row.items.into_iter().map(|item| view! { <ShelfCard item/> }).collect_view()}
                            </div>
                        }
                    })
                    .collect_view()}
            }
            .into_any(),
        }}
    }
}

#[component]
fn ShelfCard(item: DiscoverItem) -> impl IntoView {
    let nav = nav();
    let (id, movie) = (item.id, item.movie);
    let preview = Preview::new(&item.title, &item.poster);
    let open = move |_| {
        if movie {
            nav.open_movie(id, preview.clone())
        } else {
            nav.open_show(id, preview.clone())
        }
    };
    view! {
        <button class="card shelf-card" on:click=open>
            <div class="poster-wrap">
                <Poster src=item.poster/>
                {item.match_pct.map(|pct| view! { <span class="match">{format!("{pct}%")}</span> })}
            </div>
            <span class="card-title">{item.title}</span>
            <span class="card-note">{item.year.map(|y| y.to_string()).unwrap_or_default()}</span>
        </button>
    }
}

/// "What are you in the mood for?" box: the configured AI suggests shows or
/// movies (following the switch), from Discover's rows or anything else it
/// knows that Trakt has, and says why.
#[component]
fn AskAi() -> impl IntoView {
    let nav = nav();
    let configured = RwSignal::new(None::<bool>);
    let mood = RwSignal::new(String::new());
    let picks = RwSignal::new(None::<Vec<AiPick>>);
    let error = RwSignal::new(None::<String>);
    let asking = RwSignal::new(false);
    // Seconds waited; with thinking turned up it can take a minute or more.
    let elapsed = RwSignal::new(0u32);
    // Each ask (and each Shows/Movies switch) gets a new number; an answer or
    // timer from an older one is ignored.
    let generation = StoredValue::new(0u32);

    // Picks for shows don't belong under Movies, and the other way round. A
    // question still out for the other side is let go.
    Effect::new(move |previous: Option<Media>| {
        let media = nav.media.get();
        if previous.is_some_and(|p| p != media) {
            generation.update_value(|g| *g += 1);
            picks.set(None);
            error.set(None);
            asking.set(false);
        }
        media
    });

    // Re-check when settings change (e.g. a key was just saved).
    Effect::new(move |_| {
        nav.changed.track();
        spawn_local(async move {
            configured.set(Some(load_ai_settings().await.is_ok_and(|s| s.configured)));
        });
    });

    let ask = move || {
        #[derive(Serialize)]
        struct Args {
            mood: String,
            movies: bool,
        }
        let text = mood.get_untracked();
        let media = nav.media.get_untracked();
        if text.trim().is_empty() || asking.get_untracked() {
            return;
        }
        generation.update_value(|g| *g += 1);
        let current = generation.get_value();
        let is_current = move || generation.get_value() == current;
        asking.set(true);
        error.set(None);
        elapsed.set(0);
        spawn_local(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(1000).await;
                if !is_current() || !asking.get_untracked() {
                    break;
                }
                elapsed.update(|s| *s += 1);
            }
        });
        spawn_local(async move {
            let args = Args {
                mood: text,
                movies: media == Media::Movies,
            };
            let result = api::call::<Vec<AiPick>>("ai_picks", args).await;
            if !is_current() {
                return;
            }
            asking.set(false);
            match result {
                Ok(list) => picks.set(Some(list)),
                Err(e) => nav.report(e, error),
            }
        });
    };

    move || {
        match configured.get() {
        None => ().into_any(),
        Some(false) => view! {
            <div class="ai-box">
                <p class="show">"AI picks"</p>
                <p class="hint small">
                    "Describe a mood and an AI suggests shows or movies for you and says why. Add a DeepSeek (or other OpenAI-compatible) API key to turn it on."
                </p>
                <button class="pill" on:click=move |_| nav.open_settings()>"Add a key in Settings"</button>
            </div>
        }
        .into_any(),
        Some(true) => view! {
            <div class="ai-box">
                <form
                    class="inline-form"
                    on:submit=move |ev| {
                        ev.prevent_default();
                        ask();
                    }
                >
                    <input
                        type="text"
                        enterkeyhint="send"
                        placeholder="What are you in the mood for?"
                        prop:value=move || mood.get()
                        on:input=move |ev| mood.set(event_target_value(&ev))
                    />
                    <button class="pill on" type="submit" disabled=move || asking.get() || mood.get().trim().is_empty()>
                        {move || {
                            if asking.get() { format!("Thinking… {}s", elapsed.get()) } else { "Ask AI".to_string() }
                        }}
                    </button>
                </form>
                {move || error.get().map(|e| view! { <p class="warn">{e}</p> })}
                {move || {
                    picks.get().map(|list| {
                        view! {
                            <ul class="list">
                                {list.into_iter().map(|pick| view! { <AiPickRow pick/> }).collect_view()}
                            </ul>
                        }
                    })
                }}
            </div>
        }
        .into_any(),
    }
    }
}

#[component]
fn AiPickRow(pick: AiPick) -> impl IntoView {
    let nav = nav();
    let (id, movie) = (pick.id, pick.movie);
    let meta = [
        pick.year.map(|y| y.to_string()),
        pick.match_pct.map(|m| format!("{m}% match")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let preview = Preview::new(&pick.title, &pick.poster);
    view! {
        <li class="row">
            <div
                class="tap"
                on:click=move |_| {
                    if movie {
                        nav.open_movie(id, preview.clone())
                    } else {
                        nav.open_show(id, preview.clone())
                    }
                }
            >
                <Poster src=pick.poster class="thumb"/>
                <div class="info">
                    <p class="show">{pick.title}</p>
                    <p class="meta">{meta}</p>
                    <p class="reason">{pick.reason}</p>
                </div>
            </div>
        </li>
    }
}
