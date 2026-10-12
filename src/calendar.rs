use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{self, CalendarItem};
use crate::app::{Poster, nav};

/// Episodes airing in the next month, grouped by local day.
#[component]
pub fn Calendar() -> impl IntoView {
    let nav = nav();
    let days = RwSignal::new(None::<Vec<(String, Vec<CalendarItem>)>>);
    let error = RwSignal::new(None::<String>);

    Effect::new(move |_| {
        nav.changed.track();
        spawn_local(async move {
            match api::call::<Vec<CalendarItem>>("calendar", ()).await {
                Ok(items) => days.set(Some(group_by_day(items))),
                Err(e) => nav.report(e, error),
            }
        });
    });

    view! {
        {move || error.get().map(|e| view! { <p class="warn">{e}</p> })}
        {move || match days.get() {
            None => view! { <p class="hint">"Loading…"</p> }.into_any(),
            Some(days) if days.is_empty() => {
                view! { <p class="hint">"Nothing airing in the next month."</p> }.into_any()
            }
            Some(days) => days
                .into_iter()
                .map(|(label, items)| {
                    view! {
                        <h2 class="section">{label}</h2>
                        <ul class="list">
                            {items.into_iter().map(|item| view! { <CalendarRow item/> }).collect_view()}
                        </ul>
                    }
                })
                .collect_view()
                .into_any(),
        }}
    }
}

#[component]
fn CalendarRow(item: CalendarItem) -> impl IntoView {
    let nav = nav();
    let show_id = item.show_id;
    let title = item.episode.title.clone().unwrap_or_default();
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
                <Poster src=item.poster class="thumb"/>
                <div class="info">
                    <p class="show">{item.show_title}</p>
                    <p class="episode">
                        <span class="ep-code">{item.episode.code()}</span>
                        " "
                        {title}
                    </p>
                    <p class="meta">{api::short_time(&item.first_aired)}</p>
                </div>
            </div>
        </li>
    }
}

/// Groups by local day ("Today", "Tomorrow", "Fri 16 Oct"), dropping anything
/// before today in the device's time zone.
fn group_by_day(mut items: Vec<CalendarItem>) -> Vec<(String, Vec<CalendarItem>)> {
    // RFC 3339 UTC timestamps sort chronologically as strings.
    items.sort_by(|a, b| a.first_aired.cmp(&b.first_aired));
    let now = js_sys::Date::new_0();
    let today = api::local_day(&now);
    let tomorrow = {
        let d = js_sys::Date::new_0();
        d.set_date(d.get_date() + 1);
        api::local_day(&d)
    };

    let mut days: Vec<(u32, String, Vec<CalendarItem>)> = Vec::new();
    for item in items {
        let day = api::local_day(&api::date(&item.first_aired));
        if day < today {
            continue;
        }
        match days.last_mut() {
            Some((d, _, list)) if *d == day => list.push(item),
            _ => {
                let label = match day {
                    d if d == today => "Today".to_string(),
                    d if d == tomorrow => "Tomorrow".to_string(),
                    _ => api::short_date(&item.first_aired),
                };
                days.push((day, label, vec![item]));
            }
        }
    }
    days.into_iter()
        .map(|(_, label, list)| (label, list))
        .collect()
}
