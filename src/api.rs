//! Bridge to the Rust backend (Tauri commands), the data it returns, and small
//! browser helpers.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], catch)]
    async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;
}

/// Error string the backend returns when the user has to sign in (again).
pub const NOT_SIGNED_IN: &str = "not_signed_in";

/// Calls a Tauri command. Tauri expects camelCase argument names.
pub async fn call<T: DeserializeOwned>(cmd: &str, args: impl Serialize) -> Result<T, String> {
    let args = serde_wasm_bindgen::to_value(&args).map_err(|e| e.to_string())?;
    let value = invoke(cmd, args).await.map_err(|e| {
        e.as_string()
            .unwrap_or_else(|| "Something went wrong".into())
    })?;
    serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string())
}

/// Opens a Trakt or Nextep release link in the browser.
pub async fn open_link(url: &str) {
    #[derive(Serialize)]
    struct Args<'a> {
        url: &'a str,
    }
    let _ = call::<()>("open_link", Args { url }).await;
}

pub async fn mark_watched(episode_id: u64) -> Result<(), String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args {
        episode_id: u64,
    }
    call("mark_watched", Args { episode_id }).await
}

pub async fn set_show_status(show_id: u64, status: Option<ShowStatus>) -> Result<(), String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args {
        show_id: u64,
        status: Option<ShowStatus>,
    }
    call("set_show_status", Args { show_id, status }).await
}

pub async fn set_watchlist(show_id: u64, on: bool) -> Result<(), String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args {
        show_id: u64,
        on: bool,
    }
    call("set_watchlist", Args { show_id, on }).await
}

pub async fn create_list(name: &str) -> Result<ListSummary, String> {
    #[derive(Serialize)]
    struct Args<'a> {
        name: &'a str,
    }
    call("create_list", Args { name }).await
}

pub async fn set_in_list(list_id: u64, show_id: u64, on: bool) -> Result<(), String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args {
        list_id: u64,
        show_id: u64,
        on: bool,
    }
    call(
        "set_in_list",
        Args {
            list_id,
            show_id,
            on,
        },
    )
    .await
}

// ---- data (only the fields the UI uses) ------------------------------------

#[derive(Clone, Deserialize)]
pub struct Release {
    pub version: String,
    pub page_url: String,
    pub apk_url: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct Status {
    pub version: String,
    pub trakt_configured: bool,
    pub signed_in: bool,
}

#[derive(Clone, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_in: u32,
    pub interval: u32,
}

#[derive(Clone, Default, Deserialize)]
pub struct Ids {
    pub trakt: Option<u64>,
}

#[derive(Clone, Default, Deserialize)]
pub struct Show {
    pub title: Option<String>,
    pub year: Option<i32>,
    pub ids: Ids,
    pub overview: Option<String>,
    pub network: Option<String>,
    pub status: Option<String>,
    pub runtime: Option<u32>,
    pub rating: Option<f64>,
}

impl Show {
    pub fn display_title(&self) -> String {
        self.title.clone().unwrap_or_else(|| "Untitled".into())
    }
}

#[derive(Clone, Deserialize)]
pub struct Episode {
    pub season: u32,
    pub number: u32,
    pub title: Option<String>,
    pub ids: Ids,
    pub first_aired: Option<String>,
}

impl Episode {
    pub fn code(&self) -> String {
        format!("S{:02}E{:02}", self.season, self.number)
    }
}

#[derive(Clone, Deserialize)]
pub struct Item {
    pub show: Show,
    pub aired: u32,
    pub completed: u32,
    pub next_episode: Option<Episode>,
    pub poster: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct ProgressView {
    pub new_episodes: Vec<Item>,
    pub upcoming: Vec<Item>,
    pub watching: Vec<Item>,
}

#[derive(Clone, Deserialize)]
pub struct CalendarItem {
    pub first_aired: String,
    pub show_id: Option<u64>,
    pub show_title: String,
    pub episode: Episode,
    pub poster: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct HistoryItem {
    pub watched_at: String,
    pub show_id: Option<u64>,
    pub show_title: String,
    pub episode: Episode,
    pub poster: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct ListSummary {
    pub id: u64,
    pub name: String,
    pub item_count: u32,
    pub managed_by: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct ListsView {
    pub lists: Vec<ListSummary>,
    pub limit: Option<u32>,
}

#[derive(Clone, Deserialize)]
pub struct ListDetail {
    pub list: ListSummary,
    pub shows: Vec<Card>,
}

#[derive(Clone, Deserialize)]
pub struct ShowListEntry {
    pub id: u64,
    pub name: String,
    pub contains: bool,
}

#[derive(Clone, Deserialize)]
pub struct Stats {
    pub episodes: u64,
    pub minutes: u64,
    pub shows: u64,
    pub top_genres: Vec<String>,
}

#[derive(Clone, Deserialize)]
pub struct Provider {
    pub name: String,
    pub logo: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct Card {
    pub show_id: u64,
    pub title: String,
    pub poster: Option<String>,
    pub note: String,
}

#[derive(Clone, Deserialize)]
pub struct LibraryView {
    pub watching: Vec<Card>,
    pub up_to_date: Vec<Card>,
    pub finished: Vec<Card>,
    pub on_hold: Vec<Card>,
    pub dropped: Vec<Card>,
    pub watchlist: Vec<Card>,
}

#[derive(Clone, Deserialize)]
pub struct SearchItem {
    pub show_id: u64,
    pub title: String,
    pub year: Option<i32>,
    pub network: Option<String>,
    pub poster: Option<String>,
    pub watched: bool,
    pub on_watchlist: bool,
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShowStatus {
    OnHold,
    Dropped,
}

#[derive(Clone, Deserialize)]
pub struct EpisodeRow {
    pub number: u32,
    pub title: Option<String>,
    pub trakt_id: Option<u64>,
    pub first_aired: Option<String>,
    pub watched: bool,
}

#[derive(Clone, Deserialize)]
pub struct SeasonRow {
    pub number: u32,
    pub aired: u32,
    pub completed: u32,
    pub episodes: Vec<EpisodeRow>,
}

#[derive(Clone, Deserialize)]
pub struct ShowDetails {
    pub show: Show,
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub status: Option<ShowStatus>,
    pub on_watchlist: bool,
    pub aired: u32,
    pub completed: u32,
    pub next_episode: Option<Episode>,
    pub seasons: Vec<SeasonRow>,
    pub streaming: Vec<Provider>,
}

// ---- browser helpers --------------------------------------------------------

fn get(target: &JsValue, key: &str) -> Option<JsValue> {
    js_sys::Reflect::get(target, &key.into())
        .ok()
        .filter(|v| !v.is_undefined() && !v.is_null())
}

/// Calls `target.method(args...)`, ignoring failures.
fn call_method(target: &JsValue, method: &str, args: &[JsValue]) -> Option<JsValue> {
    let f = js_sys::Function::from(get(target, method)?);
    let args: js_sys::Array = args.iter().collect();
    f.apply(target, &args).ok()
}

fn global(key: &str) -> Option<JsValue> {
    get(&js_sys::global(), key)
}

/// True while the app is in the background (`document.hidden`).
pub fn page_hidden() -> bool {
    global("document")
        .and_then(|doc| get(&doc, "hidden"))
        .and_then(|h| h.as_bool())
        .unwrap_or(false)
}

/// Best effort: the code is also shown on screen if the clipboard is blocked.
pub fn copy_to_clipboard(text: &str) {
    if let Some(clipboard) = global("navigator").and_then(|nav| get(&nav, "clipboard")) {
        call_method(&clipboard, "writeText", &[text.into()]);
    }
}

/// Adds a history entry so Android's back gesture closes an overlay.
pub fn push_history() {
    if let Some(history) = global("history") {
        call_method(&history, "pushState", &[JsValue::NULL, "".into()]);
    }
}

pub fn history_back() {
    if let Some(history) = global("history") {
        call_method(&history, "back", &[]);
    }
}

/// Per-device preferences such as the chosen sort. Missing storage is fine.
pub fn load_pref(key: &str) -> Option<String> {
    let storage = global("localStorage")?;
    call_method(&storage, "getItem", &[key.into()])?.as_string()
}

pub fn save_pref(key: &str, value: &str) {
    if let Some(storage) = global("localStorage") {
        call_method(&storage, "setItem", &[key.into(), value.into()]);
    }
}

/// Country for streaming availability, from the device language ("en-AU"
/// → "AU"). Falls back to the US.
pub fn region() -> String {
    language()
        .rsplit(['-', '_'])
        .next()
        .filter(|r| r.len() == 2 && r.chars().all(|c| c.is_ascii_alphabetic()))
        .map(|r| r.to_uppercase())
        .unwrap_or_else(|| "US".into())
}

fn language() -> String {
    global("navigator")
        .and_then(|nav| get(&nav, "language"))
        .and_then(|lang| lang.as_string())
        .unwrap_or_else(|| "en".into())
}

fn options(pairs: &[(&str, &str)]) -> JsValue {
    let obj = js_sys::Object::new();
    for (k, v) in pairs {
        let _ = js_sys::Reflect::set(&obj, &(*k).into(), &(*v).into());
    }
    obj.into()
}

pub fn date(iso: &str) -> js_sys::Date {
    js_sys::Date::new(&iso.into())
}

/// "Fri 16 Oct" in the device's language and time zone.
pub fn short_date(iso: &str) -> String {
    let opts = options(&[("weekday", "short"), ("day", "numeric"), ("month", "short")]);
    date(iso).to_locale_date_string(&language(), &opts).into()
}

/// "8:00 pm" in the device's language and time zone.
pub fn short_time(iso: &str) -> String {
    let opts = options(&[("hour", "numeric"), ("minute", "2-digit")]);
    date(iso)
        .to_locale_time_string_with_options(&language(), &opts)
        .into()
}

/// Local calendar day as a sortable number (yyyymmdd).
pub fn local_day(d: &js_sys::Date) -> u32 {
    d.get_full_year() * 10_000 + (d.get_month() + 1) * 100 + d.get_date()
}
