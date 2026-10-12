//! Movie commands: what to watch, coming soon, watched, watchlist, and movie
//! pages.

use std::cmp::Reverse;
use std::collections::HashSet;

use nextep_core::chrono::{DateTime, Utc};
use nextep_core::models::{Movie, ShowPeople};
use nextep_core::tmdb::{Media, Provider, Size};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::{AppState, to_ui_error};

#[derive(Clone, Serialize, Deserialize)]
pub struct MovieCard {
    movie_id: u64,
    title: String,
    year: Option<i32>,
    poster: Option<String>,
    note: String,
}

#[derive(Default, Serialize, Deserialize)]
pub struct MoviesView {
    /// On the watchlist, released, not watched yet: most recently added first.
    to_watch: Vec<MovieCard>,
    /// On the watchlist, not released yet: soonest first, unknown dates last.
    coming_soon: Vec<MovieCard>,
    /// Most recently watched first.
    watched: Vec<MovieCard>,
    /// The whole watchlist, most recently added first.
    watchlist: Vec<MovieCard>,
}

#[tauri::command]
pub async fn movies(
    state: State<'_, AppState>,
    force: Option<bool>,
    cached: Option<bool>,
) -> Result<MoviesView, String> {
    if cached.unwrap_or(false) {
        return state.snapshot("movies");
    }
    let client = state.signed_in_client().await?;
    let (mut watched, mut watchlist) = state.movie_data(&client, force.unwrap_or(false)).await?;
    watched.sort_by_key(|w| Reverse(w.last_watched_at));
    watchlist.sort_by_key(|w| Reverse(w.listed_at));

    let today = Utc::now().date_naive();
    let seen: HashSet<u64> = watched.iter().filter_map(|w| w.movie.ids.trakt).collect();
    let unwatched: Vec<&Movie> = watchlist
        .iter()
        .map(|w| &w.movie)
        .filter(|m| m.ids.trakt.is_some_and(|id| !seen.contains(&id)))
        .collect();
    let to_watch: Vec<&Movie> = unwatched
        .iter()
        .copied()
        .filter(|m| m.released_on().is_some_and(|d| d <= today))
        .collect();
    let mut coming_soon: Vec<&Movie> = unwatched
        .iter()
        .copied()
        .filter(|m| m.released_on().is_none_or(|d| d > today))
        .collect();
    coming_soon.sort_by_key(|m| (m.released_on().is_none(), m.released_on()));

    state
        .fetch_movie_artwork(
            watched
                .iter()
                .map(|w| &w.movie)
                .chain(watchlist.iter().map(|w| &w.movie))
                .filter_map(|m| m.ids.tmdb),
        )
        .await;
    let artwork = state.artwork.lock().await;
    let card = |movie: &Movie, note: String| {
        Some(MovieCard {
            movie_id: movie.ids.trakt?,
            title: movie.display_title().to_owned(),
            year: movie.year,
            poster: artwork.poster(Media::Movie, movie.ids.tmdb, Size::Thumb),
            note,
        })
    };
    let view = MoviesView {
        to_watch: to_watch
            .iter()
            .filter_map(|m| card(m, runtime_note(m)))
            .collect(),
        coming_soon: coming_soon
            .iter()
            .filter_map(|m| card(m, release_note(m)))
            .collect(),
        watched: watched
            .iter()
            .filter_map(|w| {
                let note = match (w.plays, w.last_watched_at) {
                    (n, _) if n > 1 => format!("Watched {n} times"),
                    (_, Some(at)) => format!("Watched {}", at.format("%-d %b %Y")),
                    _ => "Watched".into(),
                };
                card(&w.movie, note)
            })
            .collect(),
        watchlist: watchlist
            .iter()
            .filter_map(|w| card(&w.movie, release_or_year(&w.movie)))
            .collect(),
    };
    drop(artwork);
    state.save_snapshot("movies", &view);
    Ok(view)
}

fn runtime_note(movie: &Movie) -> String {
    match movie.runtime {
        Some(minutes) if minutes >= 60 => format!("{}h {:02}m", minutes / 60, minutes % 60),
        Some(minutes) if minutes > 0 => format!("{minutes}m"),
        _ => movie.year.map(|y| y.to_string()).unwrap_or_default(),
    }
}

fn release_note(movie: &Movie) -> String {
    match movie.released_on() {
        Some(date) => format!("Out {}", date.format("%-d %b %Y")),
        None => "Release date TBA".into(),
    }
}

fn release_or_year(movie: &Movie) -> String {
    let today = Utc::now().date_naive();
    match movie.released_on() {
        Some(date) if date > today => release_note(movie),
        _ => movie.year.map(|y| y.to_string()).unwrap_or_default(),
    }
}

#[derive(Serialize, Deserialize)]
pub struct MovieDetails {
    movie: Movie,
    poster: Option<String>,
    backdrop: Option<String>,
    plays: u32,
    last_watched_at: Option<DateTime<Utc>>,
    on_watchlist: bool,
    released: bool,
    streaming: Vec<Provider>,
    directors: Vec<String>,
    cast: Vec<String>,
}

#[tauri::command]
/// `cached` returns the copy saved when the page was last opened.
pub async fn movie_details(
    state: State<'_, AppState>,
    movie_id: u64,
    region: Option<String>,
    cached: Option<bool>,
) -> Result<MovieDetails, String> {
    let snapshot = format!("movie_{movie_id}");
    if cached.unwrap_or(false) {
        return state.snapshot(&snapshot);
    }
    let client = state.signed_in_client().await?;
    let (movie, people) = futures::join!(client.movie(movie_id), client.movie_people(movie_id));
    let movie = movie.map_err(to_ui_error)?;
    let people = people.unwrap_or_default();
    let (watched, watchlist) = state.movie_data(&client, false).await?;
    let history = watched.iter().find(|w| w.movie.ids.trakt == Some(movie_id));

    let streaming = match (state.tmdb(), movie.ids.tmdb, region.as_deref()) {
        (Some(tmdb), Some(tmdb_id), Some(region)) => tmdb
            .streaming(Media::Movie, tmdb_id, region)
            .await
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    state.fetch_movie_artwork(movie.ids.tmdb).await;
    let (poster, backdrop) = {
        let artwork = state.artwork.lock().await;
        (
            artwork.poster(Media::Movie, movie.ids.tmdb, Size::Poster),
            artwork.backdrop(Media::Movie, movie.ids.tmdb),
        )
    };

    let details = MovieDetails {
        poster,
        backdrop,
        plays: history.map_or(0, |w| w.plays),
        last_watched_at: history.and_then(|w| w.last_watched_at),
        on_watchlist: watchlist
            .iter()
            .any(|w| w.movie.ids.trakt == Some(movie_id)),
        released: movie
            .released_on()
            .is_some_and(|d| d <= Utc::now().date_naive()),
        streaming,
        directors: directors(&people),
        cast: people
            .cast
            .iter()
            .flatten()
            .take(8)
            .map(|c| c.person.name.clone())
            .collect(),
        movie,
    };
    state.save_snapshot(&snapshot, &details);
    Ok(details)
}

/// People credited as "Director"; the directing department also lists
/// assistant directors.
fn directors(people: &ShowPeople) -> Vec<String> {
    people
        .crew
        .as_ref()
        .and_then(|c| c.get("directing"))
        .into_iter()
        .flatten()
        .filter(|m| m.jobs.iter().any(|j| j == "Director"))
        .map(|m| m.person.name.clone())
        .take(3)
        .collect()
}

#[tauri::command]
pub async fn mark_movie_watched(state: State<'_, AppState>, movie_id: u64) -> Result<(), String> {
    let client = state.signed_in_client().await?;
    client
        .mark_movie_watched(movie_id, Utc::now())
        .await
        .map_err(to_ui_error)?;
    state.invalidate_movies().await;
    Ok(())
}

#[tauri::command]
pub async fn set_movie_watchlist(
    state: State<'_, AppState>,
    movie_id: u64,
    on: bool,
) -> Result<(), String> {
    let client = state.signed_in_client().await?;
    if on {
        client.add_movie_to_watchlist(movie_id).await
    } else {
        client.remove_movie_from_watchlist(movie_id).await
    }
    .map_err(to_ui_error)?;
    state.invalidate_movies().await;
    Ok(())
}
