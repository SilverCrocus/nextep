//! Bakes API keys into the app at compile time.
//!
//! Keys come from the environment (CI) or from `.env` in the repository root
//! (local builds). `.env` is gitignored; see `.env.example`.

const KEYS: [&str; 2] = ["TRAKT_CLIENT_ID", "TMDB_API_KEY"];

fn main() {
    println!("cargo:rerun-if-changed=../.env");
    for key in KEYS {
        println!("cargo:rerun-if-env-changed={key}");
    }

    if let Ok(dotenv) = std::fs::read_to_string("../.env") {
        for line in dotenv.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim().trim_matches('"'));
            if KEYS.contains(&key) && !value.is_empty() && std::env::var_os(key).is_none() {
                println!("cargo:rustc-env={key}={value}");
            }
        }
    }

    tauri_build::build()
}
