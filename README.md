# Nextep

A lean, open-source TV show tracker for Android, written in Rust and synced
with [Trakt](https://trakt.tv).

Nextep answers one question well: **what's the next episode I should watch?**
It reads your watch history from Trakt, so anything another app scrobbles to
Trakt (for example Stremio) shows up automatically.

> **Status:** early development. The project skeleton, Trakt client and
> progress ordering exist; the app does not sign in or show data yet. See
> [docs/ROADMAP.md](docs/ROADMAP.md).

## Why

[Showly](https://github.com/trakt/showly) is a great Trakt client but has grown
large: news feeds, comments, premium tiers, analytics, dozens of settings.
Nextep keeps the parts used every day and drops the rest.

## Stack

| Layer | Tech |
|-------|------|
| Core logic (Trakt API, models, progress ordering) | Rust, [`crates/nextep-core`](crates/nextep-core) |
| App shell (Android packaging, native bridge) | [Tauri 2](https://tauri.app), [`src-tauri`](src-tauri) |
| UI | [Leptos](https://leptos.dev) compiled to WebAssembly, [`src`](src) |

The core crate has no UI or platform code, so its logic is unit tested on any
machine with `cargo test -p nextep-core`.

## Building

### Prerequisites

- Rust (stable) with targets:
  `rustup target add wasm32-unknown-unknown aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android`
- `cargo install tauri-cli --version '^2' --locked`
- `cargo install trunk --locked`
- JDK 17+, Android SDK and NDK, with `JAVA_HOME`, `ANDROID_HOME` and
  `NDK_HOME` set. See [Tauri's Android prerequisites](https://tauri.app/start/prerequisites/#android).

### API keys

Copy `.env.example` to `.env` and fill in:

- **Trakt**: create an app at <https://trakt.tv/oauth/applications/new> with
  redirect URI `urn:ietf:wg:oauth:2.0:oob`.
- **TMDB** (artwork): <https://www.themoviedb.org/settings/api>.

`.env` is gitignored and baked into the build by `src-tauri/build.rs`.

### Run on a phone

With USB debugging enabled and the phone connected:

```sh
cargo tauri android build --debug --apk --target aarch64
adb install -r src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
```

The generated Android project lives in `src-tauri/gen/android` and is
committed, so `cargo tauri android init` is not needed after cloning.

## License

[GPL-3.0-or-later](LICENSE). Nextep is inspired by Showly, which is also
GPL-3.0; any logic ported from Showly stays under the same license.
