use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;

use crate::api::{self, Item, ProgressView};
use crate::app::{Poster, Preview, nav};

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    New,
    Upcoming,
    Watching,
}

#[component]
pub fn Progress() -> impl IntoView {
    let nav = nav();
    let data = RwSignal::new(None::<ProgressView>);
    let error = RwSignal::new(None::<String>);
    let loading = RwSignal::new(false);

    let load = move |force: bool| {
        #[derive(Serialize)]
        struct Args {
            sort: String,
            force: bool,
            cached: bool,
        }
        let args = move |cached| Args {
            sort: nav.sort.get_untracked(),
            force,
            cached,
        };
        loading.set(true);
        error.set(None);
        spawn_local(async move {
            // Show the last saved list instantly, then replace it with fresh data.
            if data.get_untracked().is_none()
                && let Ok(v) = api::call::<ProgressView>("progress", args(true)).await
            {
                data.set(Some(v));
            }
            let result = api::call::<ProgressView>("progress", args(false)).await;
            loading.set(false);
            match result {
                Ok(v) => data.set(Some(v)),
                Err(e) => nav.report(e, error),
            }
        });
    };
    // Reload whenever something changed elsewhere (e.g. on the show page)...
    Effect::new(move |_| {
        nav.changed.track();
        load(false);
    });
    // ...and from Trakt, skipping caches, on pull-to-refresh.
    Effect::new(move |first_run: Option<()>| {
        nav.refresh.track();
        if first_run.is_some() {
            load(true);
        }
    });

    let on_mark = Callback::new(move |episode_id: u64| {
        spawn_local(async move {
            match api::mark_watched(episode_id).await {
                Ok(()) => nav.data_changed(),
                Err(e) => nav.report(e, error),
            }
        });
    });

    view! {
        <div class="toolbar">
            <select
                prop:value=move || nav.sort.get()
                on:change=move |ev| {
                    let value = event_target_value(&ev);
                    api::save_pref("sort", &value);
                    nav.sort.set(value);
                    load(false);
                }
            >
                <option value="recently_watched">"Recently watched"</option>
                <option value="title">"Title"</option>
                <option value="episodes_left">"Fewest left"</option>
                <option value="newest_episode">"Newest episode"</option>
            </select>
            {move || loading.get().then(|| view! { <span class="hint small">"Updating…"</span> })}
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
                <Section title="Upcoming" kind=Kind::Upcoming items=v.upcoming on_mark/>
                <Section title="Watching" kind=Kind::Watching items=v.watching on_mark/>
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
    let nav = nav();
    let ep = item.next_episode;
    let code = ep.as_ref().map(|e| e.code()).unwrap_or_default();
    let ep_title = ep
        .as_ref()
        .and_then(|e| e.title.clone())
        .unwrap_or_default();
    let episode_id = ep.as_ref().and_then(|e| e.ids.trakt);
    let first_aired = ep.as_ref().and_then(|e| e.first_aired.clone());

    let left = item.aired.saturating_sub(item.completed);
    let meta = match (kind, first_aired) {
        (Kind::Upcoming, Some(at)) => {
            format!("{} · {}", api::short_date(&at), api::short_time(&at))
        }
        (Kind::New, Some(at)) => format!("{left} left · aired {}", api::short_date(&at)),
        _ => format!("{left} left"),
    };
    let percent = (item.completed * 100).checked_div(item.aired).unwrap_or(0);
    let marking = RwSignal::new(false);
    let show_id = item.show.ids.trakt;
    let preview = Preview::new(&item.show.display_title(), &item.poster);
    let open = move |_| {
        if let Some(id) = show_id {
            nav.open_show(id, preview.clone());
        }
    };

    view! {
        <li class="row">
            <div class="tap" on:click=open>
                <Poster src=item.poster class="thumb"/>
                <div class="info">
                    <p class="show">{item.show.display_title()}</p>
                    <p class="episode">
                        <span class="ep-code">{code}</span>
                        " "
                        {ep_title}
                    </p>
                    <p class="meta">{meta}</p>
                    {(kind != Kind::Upcoming)
                        .then(|| {
                            view! {
                                <div class="bar">
                                    <div class="fill" style=format!("width: {percent}%")></div>
                                </div>
                            }
                        })}
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
