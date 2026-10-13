//! Personal Trakt lists: browse, create, rename, delete, and add or remove
//! shows and movies. Lists that couchmoney or mdblist regenerate are
//! browse-only.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;

use crate::api::{self, ItemListEntry, ListDetail, ListSummary, ListsView};
use crate::app::{Poster, Preview, nav};

/// The Lists group in Library.
#[component]
pub fn ListsPanel() -> impl IntoView {
    let nav = nav();
    let data = RwSignal::new(None::<ListsView>);
    let error = RwSignal::new(None::<String>);

    let load = move |force: bool| {
        #[derive(Serialize)]
        struct Args {
            force: bool,
            cached: bool,
        }
        spawn_local(async move {
            if data.with_untracked(Option::is_none)
                && let Ok(v) = api::call::<ListsView>(
                    "lists",
                    Args {
                        force,
                        cached: true,
                    },
                )
                .await
            {
                data.set(Some(v));
            }
            match api::call::<ListsView>(
                "lists",
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

    let on_created = Callback::new(move |list: ListSummary| {
        nav.data_changed();
        nav.open_list(list.id);
    });

    view! {
        {move || error.get().map(|e| view! { <p class="warn">{e}</p> })}
        {move || match data.get() {
            None => view! { <p class="hint">"Loading…"</p> }.into_any(),
            Some(v) => {
                let used = v.lists.len();
                let at_limit = v.limit.is_some_and(|limit| used as u32 >= limit);
                let (managed, own): (Vec<_>, Vec<_>) = v
                    .lists
                    .into_iter()
                    .partition(|l| l.managed_by.is_some());
                view! {
                    <h2 class="section">
                        "Your lists"
                        <span class="count">
                            {v.limit.map(|limit| format!("{used} of {limit} lists")).unwrap_or_default()}
                        </span>
                    </h2>
                    {own.is_empty().then(|| view! { <p class="hint">"No lists yet."</p> })}
                    <ul class="list">{own.into_iter().map(|list| view! { <ListRow list/> }).collect_view()}</ul>
                    {if at_limit {
                        view! {
                            <p class="hint small">"Trakt's list limit is reached. Delete a list to make a new one."</p>
                        }
                            .into_any()
                    } else {
                        view! { <NewListForm on_created/> }.into_any()
                    }}
                    {(!managed.is_empty())
                        .then(|| {
                            view! {
                                <h2 class="section">"Recommendations"</h2>
                                <ul class="list">
                                    {managed.into_iter().map(|list| view! { <ListRow list/> }).collect_view()}
                                </ul>
                            }
                        })}
                }
                    .into_any()
            }
        }}
    }
}

#[component]
fn ListRow(list: ListSummary) -> impl IntoView {
    let nav = nav();
    let id = list.id;
    // Trakt counts every item, movies included.
    let titles = match list.item_count {
        1 => "1 title".to_string(),
        n => format!("{n} titles"),
    };
    let note = match &list.managed_by {
        Some(by) => format!("{titles} · by {by}"),
        None => titles,
    };
    view! {
        <li class="row">
            <button class="list-row" on:click=move |_| nav.open_list(id)>
                <span class="info">
                    <span class="show">{list.name}</span>
                    <span class="meta">{note}</span>
                </span>
                <span class="chevron">"›"</span>
            </button>
        </li>
    }
}

/// Inline "New list" field; calls `on_created` with the new list.
#[component]
fn NewListForm(on_created: Callback<ListSummary>) -> impl IntoView {
    let name = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let create = move || {
        let text = name.get_untracked();
        if text.trim().is_empty() {
            return;
        }
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match api::create_list(&text).await {
                Ok(list) => {
                    name.set(String::new());
                    on_created.run(list);
                }
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        });
    };
    view! {
        <form
            class="inline-form"
            on:submit=move |ev| {
                ev.prevent_default();
                create();
            }
        >
            <input
                type="text"
                placeholder="New list name"
                enterkeyhint="done"
                prop:value=move || name.get()
                on:input=move |ev| name.set(event_target_value(&ev))
            />
            <button class="pill" type="submit" disabled=move || busy.get() || name.get().trim().is_empty()>
                "Create"
            </button>
        </form>
        {move || error.get().map(|e| view! { <p class="warn">{e}</p> })}
    }
}

/// Full-screen page for one list.
#[component]
pub fn ListPage(list_id: u64) -> impl IntoView {
    let nav = nav();
    let data = RwSignal::new(None::<ListDetail>);
    let error = RwSignal::new(None::<String>);
    let editing = RwSignal::new(false);

    let load = move || {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Args {
            list_id: u64,
        }
        spawn_local(async move {
            match api::call::<ListDetail>("list_detail", Args { list_id }).await {
                Ok(d) => data.set(Some(d)),
                Err(e) => nav.report(e, error),
            }
        });
    };
    load();

    let on_renamed = Callback::new(move |name: String| {
        data.update(|d| {
            if let Some(d) = d {
                d.list.name = name;
            }
        });
    });

    let remove = Callback::new(move |(id, movie): (u64, bool)| {
        // Drop it from the page right away; Trakt catches up in the background.
        data.update(|d| {
            if let Some(d) = d {
                d.items.retain(|c| (c.id, c.movie) != (id, movie));
            }
        });
        spawn_local(async move {
            match api::set_in_list(list_id, id, movie, false).await {
                Ok(()) => nav.data_changed(),
                Err(e) => {
                    error.set(Some(e));
                    load();
                }
            }
        });
    });

    view! {
        <div class="sheet">
            <button class="back" aria-label="Back" on:click=move |_| nav.close()>
                "←"
            </button>
            <div class="details list-page">
                {move || error.get().map(|e| view! { <p class="warn">{e}</p> })}
                {move || match data.get() {
                    None => view! { <p class="hint center">"Loading…"</p> }.into_any(),
                    Some(d) => {
                        let own = d.list.managed_by.is_none();
                        view! {
                            <h1 class="show-title">{d.list.name.clone()}</h1>
                            <p class="meta">
                                {match &d.list.managed_by {
                                    Some(by) => format!("{} · kept up to date by {by}", count_label(d.items.len() as u32)),
                                    None => count_label(d.items.len() as u32),
                                }}
                            </p>
                            {own.then(|| view! { <ListActions list=d.list.clone() editing on_renamed/> })}
                            {d.items.is_empty().then(|| {
                                view! {
                                    <p class="hint">
                                        {if own {
                                            "Add shows and movies from their page with “Add to list”."
                                        } else {
                                            "Nothing on this list yet."
                                        }}
                                    </p>
                                }
                            })}
                            <div class="grid">
                                {d.items
                                    .into_iter()
                                    .map(|card| {
                                        let (id, movie) = (card.id, card.movie);
                                        let preview = Preview::new(&card.title, &card.poster);
                                        let open = move |_| {
                                            if movie {
                                                nav.open_movie(id, preview.clone())
                                            } else {
                                                nav.open_show(id, preview.clone())
                                            }
                                        };
                                        view! {
                                            <div class="card-wrap">
                                                <button class="card" on:click=open>
                                                    <Poster src=card.poster/>
                                                    <span class="card-title">{card.title}</span>
                                                    <span class="card-note">{card.note}</span>
                                                </button>
                                                {move || {
                                                    (own && editing.get())
                                                        .then(|| {
                                                            view! {
                                                                <button
                                                                    class="remove"
                                                                    aria-label="Remove from list"
                                                                    on:click=move |_| remove.run((id, movie))
                                                                >
                                                                    "✕"
                                                                </button>
                                                            }
                                                        })
                                                }}
                                            </div>
                                        }
                                    })
                                    .collect_view()}
                            </div>
                        }
                            .into_any()
                    }
                }}
            </div>
        </div>
    }
}

/// Edit / rename / delete for one of the user's own lists.
#[component]
fn ListActions(
    list: ListSummary,
    editing: RwSignal<bool>,
    on_renamed: Callback<String>,
) -> impl IntoView {
    let nav = nav();
    let list_id = list.id;
    let renaming = RwSignal::new(false);
    let name = RwSignal::new(list.name.clone());
    let confirm_delete = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);

    let rename = move || {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Args {
            list_id: u64,
            name: String,
        }
        let new_name = name.get_untracked();
        spawn_local(async move {
            match api::call::<()>(
                "rename_list",
                Args {
                    list_id,
                    name: new_name,
                },
            )
            .await
            {
                Ok(()) => {
                    renaming.set(false);
                    on_renamed.run(name.get_untracked());
                    nav.data_changed();
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };
    let delete = move |_| {
        if !confirm_delete.get_untracked() {
            confirm_delete.set(true);
            return;
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Args {
            list_id: u64,
        }
        spawn_local(async move {
            match api::call::<()>("delete_list", Args { list_id }).await {
                Ok(()) => {
                    nav.data_changed();
                    nav.close();
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };

    view! {
        {move || {
            if renaming.get() {
                view! {
                    <form
                        class="inline-form"
                        on:submit=move |ev| {
                            ev.prevent_default();
                            rename();
                        }
                    >
                        <input
                            type="text"
                            enterkeyhint="done"
                            prop:value=move || name.get()
                            on:input=move |ev| name.set(event_target_value(&ev))
                        />
                        <button class="pill" type="submit">"Save"</button>
                    </form>
                }
                    .into_any()
            } else {
                view! {
                    <div class="actions">
                        <button class="pill" class:on=move || editing.get() on:click=move |_| editing.update(|e| *e = !*e)>
                            {move || if editing.get() { "Done" } else { "Edit" }}
                        </button>
                        <button class="pill" on:click=move |_| renaming.set(true)>"Rename"</button>
                        <button class="pill danger" on:click=delete>
                            {move || if confirm_delete.get() { "Tap again to delete" } else { "Delete" }}
                        </button>
                    </div>
                }
                    .into_any()
            }
        }}
        {move || error.get().map(|e| view! { <p class="warn">{e}</p> })}
    }
}

/// "Add to list" on a show or movie page: the user's lists with a tick for
/// each one containing it, plus a quick "new list". `id` is the show's, or
/// with `movie` the movie's.
#[component]
pub fn ListPicker(id: u64, #[prop(optional)] movie: bool) -> impl IntoView {
    let nav = nav();
    let open = RwSignal::new(false);
    let entries = RwSignal::new(None::<Vec<ItemListEntry>>);
    let error = RwSignal::new(None::<String>);

    let load = move || {
        #[derive(Serialize)]
        struct Args {
            id: u64,
            movie: bool,
        }
        spawn_local(async move {
            match api::call::<Vec<ItemListEntry>>("item_lists", Args { id, movie }).await {
                Ok(v) => entries.set(Some(v)),
                Err(e) => nav.report(e, error),
            }
        });
    };
    let toggle = move |list_id: u64, on: bool| {
        entries.update(|list| {
            if let Some(list) = list
                && let Some(entry) = list.iter_mut().find(|e| e.id == list_id)
            {
                entry.contains = on;
            }
        });
        spawn_local(async move {
            match api::set_in_list(list_id, id, movie, on).await {
                Ok(()) => nav.data_changed(),
                Err(e) => {
                    error.set(Some(e));
                    load();
                }
            }
        });
    };
    let on_created = Callback::new(move |list: ListSummary| {
        entries.update(|l| {
            if let Some(l) = l {
                l.push(ItemListEntry {
                    id: list.id,
                    name: list.name.clone(),
                    contains: false,
                });
            }
        });
        toggle(list.id, true);
    });

    view! {
        <button
            class="pill wide"
            class:on=move || open.get()
            on:click=move |_| {
                let now_open = !open.get_untracked();
                open.set(now_open);
                if now_open {
                    load();
                }
            }
        >
            {move || if open.get() { "Close lists" } else { "Add to list" }}
        </button>
        {move || {
            open.get()
                .then(|| {
                    view! {
                        <div class="picker">
                            {move || error.get().map(|e| view! { <p class="warn">{e}</p> })}
                            {move || match entries.get() {
                                None => view! { <p class="hint">"Loading…"</p> }.into_any(),
                                Some(list) => list
                                    .into_iter()
                                    .map(|entry| {
                                        let id = entry.id;
                                        let on = entry.contains;
                                        view! {
                                            <button class="pick" class:on=on on:click=move |_| toggle(id, !on)>
                                                <span class="tick">{if on { "✓" } else { "" }}</span>
                                                {entry.name}
                                            </button>
                                        }
                                    })
                                    .collect_view()
                                    .into_any(),
                            }}
                            <NewListForm on_created/>
                        </div>
                    }
                })
        }}
    }
}

fn count_label(n: u32) -> String {
    match n {
        1 => "1 title".into(),
        n => format!("{n} titles"),
    }
}
