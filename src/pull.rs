//! Pull down at the top of a screen to refresh it.

use leptos::prelude::*;
use wasm_bindgen::JsValue;

/// How far (after resistance) the list must be pulled to refresh.
const TRIGGER_PX: f64 = 70.0;
const MAX_PX: f64 = 110.0;

#[component]
pub fn PullToRefresh(on_refresh: Callback<()>, children: Children) -> impl IntoView {
    let start = StoredValue::new(None::<f64>);
    let pull = RwSignal::new(0.0_f64);
    let reset = move || {
        start.set_value(None);
        pull.set(0.0);
    };

    view! {
        <div
            on:touchstart=move |ev: leptos::ev::TouchEvent| {
                // Only a pull that starts with the page scrolled to the top.
                let at_top = scroll_y() <= 0.0;
                start.set_value(if at_top { touch_y(ev.as_ref()) } else { None });
            }
            on:touchmove=move |ev: leptos::ev::TouchEvent| {
                if let (Some(from), Some(y)) = (start.get_value(), touch_y(ev.as_ref())) {
                    pull.set(((y - from) * 0.5).clamp(0.0, MAX_PX));
                }
            }
            on:touchend=move |_| {
                if pull.get_untracked() >= TRIGGER_PX {
                    on_refresh.run(());
                }
                reset();
            }
            on:touchcancel=move |_| reset()
        >
            <div
                class="ptr"
                style=move || {
                    format!("height: {}px; opacity: {}", pull.get(), (pull.get() / TRIGGER_PX).min(1.0))
                }
            >
                {move || if pull.get() >= TRIGGER_PX { "Release to refresh" } else { "Pull to refresh" }}
            </div>
            {children()}
        </div>
    }
}

fn touch_y(ev: &JsValue) -> Option<f64> {
    let touches = js_sys::Reflect::get(ev, &"touches".into()).ok()?;
    let first = js_sys::Reflect::get(&touches, &0.into()).ok()?;
    js_sys::Reflect::get(&first, &"clientY".into())
        .ok()?
        .as_f64()
}

fn scroll_y() -> f64 {
    js_sys::Reflect::get(&js_sys::global(), &"scrollY".into())
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
}
