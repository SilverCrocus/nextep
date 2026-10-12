use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{self, Card, LibraryView};
use crate::app::{Poster, nav};

#[derive(Clone, Copy, PartialEq)]
enum Group {
    Watching,
    UpToDate,
    Finished,
    OnHold,
    Dropped,
    Watchlist,
}

impl Group {
    const ALL: [Group; 6] = [
        Group::Watching,
        Group::UpToDate,
        Group::OnHold,
        Group::Dropped,
        Group::Finished,
        Group::Watchlist,
    ];

    fn label(self) -> &'static str {
        match self {
            Group::Watching => "Watching",
            Group::UpToDate => "Up to date",
            Group::Finished => "Finished",
            Group::OnHold => "On hold",
            Group::Dropped => "Dropped",
            Group::Watchlist => "Watchlist",
        }
    }

    fn cards(self, view: &LibraryView) -> &Vec<Card> {
        match self {
            Group::Watching => &view.watching,
            Group::UpToDate => &view.up_to_date,
            Group::Finished => &view.finished,
            Group::OnHold => &view.on_hold,
            Group::Dropped => &view.dropped,
            Group::Watchlist => &view.watchlist,
        }
    }
}

#[component]
pub fn Library() -> impl IntoView {
    let nav = nav();
    let data = RwSignal::new(None::<LibraryView>);
    let error = RwSignal::new(None::<String>);
    let group = RwSignal::new(Group::Watching);

    Effect::new(move |_| {
        nav.changed.track();
        spawn_local(async move {
            match api::call::<LibraryView>("library", ()).await {
                Ok(v) => data.set(Some(v)),
                Err(e) => nav.report(e, error),
            }
        });
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
                        let count = g.cards(&v).len();
                        view! {
                            <button class="chip" class:active=move || group.get() == g on:click=move |_| group.set(g)>
                                {g.label()}
                                <span class="count">{count}</span>
                            </button>
                        }
                    })
                    .collect_view();
                let v2 = v.clone();
                view! {
                    <div class="chips">{chips}</div>
                    {move || {
                        let cards = group.get().cards(&v2).clone();
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
        <div class="footer">
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
