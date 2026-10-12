use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::api::{self, CalendarItem, Episode, HistoryItem};
use crate::app::{Poster, nav};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Upcoming,
    History,
}

/// Upcoming episodes for the next month, or the episodes you watched
/// recently, grouped by local day.
#[component]
pub fn Calendar() -> impl IntoView {
    let mode = RwSignal::new(Mode::Upcoming);
    view! {
        <div class="chips">
            {[(Mode::Upcoming, "Upcoming"), (Mode::History, "History")]
                .into_iter()
                .map(|(m, label)| {
                    view! {
                        <button class="chip" class:active=move || mode.get() == m on:click=move |_| mode.set(m)>
                            {label}
                        </button>
                    }
                })
                .collect_view()}
        </div>
        {move || match mode.get() {
            Mode::Upcoming => view! { <Upcoming/> }.into_any(),
            Mode::History => view! { <History/> }.into_any(),
        }}
    }
}

#[component]
fn Upcoming() -> impl IntoView {
    let days = load_days::<CalendarItem>("calendar", |items| {
        // RFC 3339 UTC timestamps sort chronologically as strings.
        let mut items = items;
        items.sort_by(|a, b| a.first_aired.cmp(&b.first_aired));
        let today = api::local_day(&js_sys::Date::new_0());
        items.retain(|i| api::local_day(&api::date(&i.first_aired)) >= today);
        group_by_day(items, |i| i.first_aired.clone())
    });
    view! {
        <Days
            days
            empty="Nothing airing in the next month."
            row=|item: CalendarItem| {
                let time = api::short_time(&item.first_aired);
                view! { <EpisodeRow show_id=item.show_id title=item.show_title episode=item.episode poster=item.poster meta=time/> }
            }
        />
    }
}

#[component]
fn History() -> impl IntoView {
    let days = load_days::<HistoryItem>("history", |items| {
        // Trakt returns newest first.
        group_by_day(items, |i| i.watched_at.clone())
    });
    view! {
        <Days
            days
            empty="Nothing watched yet."
            row=|item: HistoryItem| {
                let time = format!("Watched {}", api::short_time(&item.watched_at));
                view! { <EpisodeRow show_id=item.show_id title=item.show_title episode=item.episode poster=item.poster meta=time/> }
            }
        />
    }
}

/// Items grouped under a day label, in display order.
type Grouped<T> = Vec<(String, Vec<T>)>;
type DaysSignal<T> = RwSignal<Option<Grouped<T>>>;
/// Grouped days plus the last error.
type Loaded<T> = (DaysSignal<T>, RwSignal<Option<String>>);

/// Loads `command` (saved copy first, then fresh), regrouping with `group`.
/// Reloads when data changes elsewhere or on pull-to-refresh.
fn load_days<T: DeserializeOwned + Send + Sync + 'static>(
    command: &'static str,
    group: fn(Vec<T>) -> Grouped<T>,
) -> Loaded<T> {
    let nav = nav();
    let days: DaysSignal<T> = RwSignal::new(None);
    let error = RwSignal::new(None::<String>);
    let load = move || {
        #[derive(Serialize)]
        struct Args {
            cached: bool,
        }
        error.set(None);
        spawn_local(async move {
            if days.with_untracked(Option::is_none)
                && let Ok(items) = api::call::<Vec<T>>(command, Args { cached: true }).await
            {
                days.set(Some(group(items)));
            }
            match api::call::<Vec<T>>(command, Args { cached: false }).await {
                Ok(items) => days.set(Some(group(items))),
                Err(e) => nav.report(e, error),
            }
        });
    };
    Effect::new(move |_| {
        nav.changed.track();
        nav.refresh.track();
        load();
    });
    (days, error)
}

#[component]
fn Days<T, V>(days: Loaded<T>, empty: &'static str, row: fn(T) -> V) -> impl IntoView
where
    T: Clone + Send + Sync + 'static,
    V: IntoView + 'static,
{
    let (days, error) = days;
    view! {
        {move || error.get().map(|e| view! { <p class="warn">{e}</p> })}
        {move || match days.get() {
            None => view! { <p class="hint">"Loading…"</p> }.into_any(),
            Some(days) if days.is_empty() => view! { <p class="hint">{empty}</p> }.into_any(),
            Some(days) => days
                .into_iter()
                .map(|(label, items)| {
                    view! {
                        <h2 class="section">{label}</h2>
                        <ul class="list">{items.into_iter().map(row).collect_view()}</ul>
                    }
                })
                .collect_view()
                .into_any(),
        }}
    }
}

#[component]
fn EpisodeRow(
    show_id: Option<u64>,
    title: String,
    episode: Episode,
    poster: Option<String>,
    meta: String,
) -> impl IntoView {
    let nav = nav();
    let episode_title = episode.title.clone().unwrap_or_default();
    view! {
        <li class="row">
            <div
                class="tap"
                on:click=move |_| {
                    if let Some(id) = show_id {
                        nav.open_show(id);
                    }
                }
            >
                <Poster src=poster class="thumb"/>
                <div class="info">
                    <p class="show">{title}</p>
                    <p class="episode">
                        <span class="ep-code">{episode.code()}</span>
                        " "
                        {episode_title}
                    </p>
                    <p class="meta">{meta}</p>
                </div>
            </div>
        </li>
    }
}

/// Groups consecutive items by local day: "Today", "Tomorrow", "Yesterday",
/// otherwise "Fri 16 Oct". Items must already be in display order.
fn group_by_day<T>(items: Vec<T>, when: fn(&T) -> String) -> Grouped<T> {
    let offset_day = |days: i32| {
        let d = js_sys::Date::new_0();
        d.set_date((d.get_date() as i32 + days) as u32);
        api::local_day(&d)
    };
    let (today, tomorrow, yesterday) = (offset_day(0), offset_day(1), offset_day(-1));

    let mut groups: Vec<(u32, String, Vec<T>)> = Vec::new();
    for item in items {
        let iso = when(&item);
        let day = api::local_day(&api::date(&iso));
        match groups.last_mut() {
            Some((d, _, list)) if *d == day => list.push(item),
            _ => {
                let label = match day {
                    d if d == today => "Today".to_string(),
                    d if d == tomorrow => "Tomorrow".to_string(),
                    d if d == yesterday => "Yesterday".to_string(),
                    _ => api::short_date(&iso),
                };
                groups.push((day, label, vec![item]));
            }
        }
    }
    groups
        .into_iter()
        .map(|(_, label, list)| (label, list))
        .collect()
}
