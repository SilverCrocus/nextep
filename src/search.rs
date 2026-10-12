use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;

use crate::api::{self, SearchItem};
use crate::app::{Poster, nav};

/// Search Trakt for shows; open one or add it to the watchlist.
#[component]
pub fn SearchPage() -> impl IntoView {
    let nav = nav();
    let query = RwSignal::new(String::new());
    let results = RwSignal::new(None::<Vec<SearchItem>>);
    let error = RwSignal::new(None::<String>);
    let searching = RwSignal::new(false);
    // Only the latest search may update the results.
    let generation = StoredValue::new(0u32);
    let input = NodeRef::<leptos::html::Input>::new();

    Effect::new(move |_| {
        if let Some(el) = input.get() {
            let _ = el.focus();
        }
    });

    let run = move |text: String, delay_ms: u32| {
        #[derive(Serialize)]
        struct Args {
            query: String,
        }
        let current = generation.get_value() + 1;
        generation.set_value(current);
        spawn_local(async move {
            if delay_ms > 0 {
                // Wait for the user to stop typing.
                TimeoutFuture::new(delay_ms).await;
            }
            if generation.get_value() != current {
                return;
            }
            if text.trim().chars().count() < 2 {
                results.set(None);
                return;
            }
            searching.set(true);
            error.set(None);
            let result = api::call::<Vec<SearchItem>>("search", Args { query: text }).await;
            if generation.get_value() != current {
                return;
            }
            searching.set(false);
            match result {
                Ok(items) => results.set(Some(items)),
                Err(e) => nav.report(e, error),
            }
        });
    };

    view! {
        <div class="sheet search">
            <div class="search-bar">
                <button class="icon" aria-label="Back" on:click=move |_| nav.close()>
                    "←"
                </button>
                <input
                    node_ref=input
                    type="search"
                    placeholder="Search shows"
                    enterkeyhint="search"
                    autocomplete="off"
                    prop:value=move || query.get()
                    on:input=move |ev| {
                        let text = event_target_value(&ev);
                        query.set(text.clone());
                        run(text, 350);
                    }
                    on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                        if ev.key() == "Enter" {
                            run(query.get_untracked(), 0);
                        }
                    }
                />
            </div>
            <div class="content">
                {move || error.get().map(|e| view! { <p class="warn">{e}</p> })}
                {move || match results.get() {
                    None if searching.get() => view! { <p class="hint">"Searching…"</p> }.into_any(),
                    None => view! { <p class="hint">"Find a show to add to your watchlist."</p> }.into_any(),
                    Some(items) if items.is_empty() => view! { <p class="hint">"No shows found."</p> }.into_any(),
                    Some(items) => view! {
                        <ul class="list">
                            {items.into_iter().map(|item| view! { <SearchRow item/> }).collect_view()}
                        </ul>
                    }
                    .into_any(),
                }}
            </div>
        </div>
    }
}

#[component]
fn SearchRow(item: SearchItem) -> impl IntoView {
    let nav = nav();
    let show_id = item.show_id;
    let on_watchlist = RwSignal::new(item.on_watchlist);
    let busy = RwSignal::new(false);
    let meta = [item.year.map(|y| y.to_string()), item.network]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");

    let toggle = move |_| {
        let on = !on_watchlist.get_untracked();
        busy.set(true);
        spawn_local(async move {
            if api::set_watchlist(show_id, on).await.is_ok() {
                on_watchlist.set(on);
                nav.data_changed();
            }
            busy.set(false);
        });
    };

    view! {
        <li class="row">
            <div class="tap" on:click=move |_| nav.open_show(show_id)>
                <Poster src=item.poster class="thumb"/>
                <div class="info">
                    <p class="show">{item.title}</p>
                    <p class="meta">{meta}</p>
                    {item.watched.then(|| view! { <p class="meta accent">"In your library"</p> })}
                </div>
            </div>
            {(!item.watched)
                .then(move || {
                    view! {
                        <button
                            class="pill"
                            class:on=move || on_watchlist.get()
                            disabled=move || busy.get()
                            on:click=toggle
                        >
                            {move || if on_watchlist.get() { "✓ Watchlist" } else { "+ Watchlist" }}
                        </button>
                    }
                })}
        </li>
    }
}
