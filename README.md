# Nextep

A lean, open-source TV show tracker for Android, written in Rust and synced
with [Trakt](https://trakt.tv).

Nextep answers one question well: **what's the next episode I should watch?**
It reads your watch history from Trakt, so anything another app scrobbles to
Trakt (for example Stremio) shows up automatically.

> **Status:** early development, usable day to day: Trakt sign-in, Progress,
> Calendar, Library, show pages, posters, and On hold / Dropped labels. See
> [docs/ROADMAP.md](docs/ROADMAP.md).

## Install

Nextep isn't on the Play Store. Either:

- **Obtainium (recommended, keeps Nextep up to date):** install
  [Obtainium](https://github.com/ImranR98/Obtainium), tap **Add App**, paste
  `https://github.com/SilverCrocus/nextep` and install. Obtainium watches for
  new releases and can install them in the background on Android 12 and
  newer.
- **Manually:** download the latest `nextep-x.y.z.apk` from
  [Releases](https://github.com/SilverCrocus/nextep/releases). Nextep shows a
  banner when a newer version is out.

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

- **Trakt**: create an app at <https://developer.trakt.tv/apps>. Only the
  Client ID is used. If you fork, register your own redirect URI and update
  `REDIRECT_URI` in `crates/nextep-core/src/trakt/mod.rs`.
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

## Releasing

```sh
scripts/release.sh 0.3.0
```

This bumps the version in `Cargo.toml` and `tauri.conf.json`, commits, tags
`v0.3.0` and pushes. [The release workflow](.github/workflows/release.yml)
then builds a signed APK and publishes it on GitHub, where Obtainium and the
in-app update check find it.

Every release must be signed with the same key, or Android refuses to install
it as an update. The key is in the repository secrets
(`ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`) and, for local
builds, `~/.android-keys/` (see `src-tauri/gen/android/app/build.gradle.kts`).
Keep a backup of it.

## License

[GPL-3.0-or-later](LICENSE). Nextep is inspired by Showly, which is also
GPL-3.0; any logic ported from Showly stays under the same license.
