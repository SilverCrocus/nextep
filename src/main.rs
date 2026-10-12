mod api;
mod app;
mod calendar;
mod library;
mod lists;
mod progress;
mod pull;
mod search;
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
