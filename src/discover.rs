use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;

use crate::api::{self, DiscoverItem, DiscoverView};
use crate::app::{Poster, nav};

/// Shows you haven't watched, in shelves: picked by the on-device taste
/// model, related to what you watched last, Trakt's picks, and trending.
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
        }
        spawn_local(async move {
            if data.with_untracked(Option::is_none)
                && let Ok(v) = api::call::<DiscoverView>(
                    "discover",
                    Args {
                        force,
                        cached: true,
                    },
                )
                .await
            {
                data.set(Some(v));
            }
            match api::call::<DiscoverView>(
                "discover",
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

    view! {
        {move || error.get().map(|e| view! { <p class="warn">{e}</p> })}
        {move || match data.get() {
            None => view! { <p class="hint">"Finding shows for you…"</p> }.into_any(),
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
    let show_id = item.show_id;
    view! {
        <button class="card shelf-card" on:click=move |_| nav.open_show(show_id)>
            <div class="poster-wrap">
                <Poster src=item.poster/>
                {item.match_pct.map(|pct| view! { <span class="match">{format!("{pct}%")}</span> })}
            </div>
            <span class="card-title">{item.title}</span>
            <span class="card-note">{item.year.map(|y| y.to_string()).unwrap_or_default()}</span>
        </button>
    }
}
