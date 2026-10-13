mod api;
mod app;
mod calendar;
mod discover;
mod library;
mod lists;
mod movies;
mod profile;
mod progress;
mod pull;
mod search;
mod settings;
mod show;

use app::App;
use leptos::prelude::*;

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(|| {
        view! {
            <App/>
        }
    })
}
