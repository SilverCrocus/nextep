//! A small "would I like it?" model trained on the user's own history.
//!
//! Runs on the device: no API, nothing leaves the phone. Shows the user
//! finished or kept watching count as liked; dropped, on hold, or abandoned
//! early count as not. A logistic regression over simple show features
//! (genres, network, decade, rating, episode length) then scores new shows.
//! Movies are scored by the same model on the features they share with
//! shows (genres, decade, rating): Trakt has no "dropped" for a movie, so
//! there is nothing to learn movie dislikes from.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};

use crate::labels::ShowStatus;
use crate::models::{Movie, Show};
use crate::progress::ProgressItem;

/// Below this many examples the model knows too little to be shown.
pub const MIN_EXAMPLES: usize = 10;

const EPOCHS: usize = 200;
const LEARNING_RATE: f64 = 0.05;
/// Strong enough that a network or genre seen on two shows can't push a
/// score to 99%.
const L2: f64 = 0.05;

/// Whether a watched show counts as liked, not liked, or unclear (`None`).
pub fn label(item: &ProgressItem, status: Option<ShowStatus>, now: DateTime<Utc>) -> Option<bool> {
    if status.is_some() {
        return Some(false);
    }
    let watched = item.completed as f64;
    let aired = item.aired.max(1) as f64;
    if item.completed >= 10 || watched / aired >= 0.6 {
        return Some(true);
    }
    // Started, barely watched, and untouched for months: abandoned.
    let stale = item
        .last_watched_at
        .is_none_or(|at| now - at > Duration::days(90));
    (item.aired >= 4 && watched / aired < 0.25 && stale).then_some(false)
}

/// Sparse features of a show, e.g. `genre:drama`, `network:hbo`.
pub fn features(show: &Show) -> Vec<String> {
    let mut out = shared_features(show.genres.as_deref(), show.year, show.rating);
    if let Some(network) = &show.network {
        out.push(format!("network:{}", network.to_lowercase()));
    }
    if let Some(runtime) = show.runtime.filter(|r| *r > 0) {
        let bucket = match runtime {
            r if r <= 30 => "short",
            r if r <= 50 => "medium",
            _ => "long",
        };
        out.push(format!("runtime:{bucket}"));
    }
    out
}

/// Features a movie shares with shows: genres, decade and rating.
pub fn movie_features(movie: &Movie) -> Vec<String> {
    shared_features(movie.genres.as_deref(), movie.year, movie.rating)
}

fn shared_features(
    genres: Option<&[String]>,
    year: Option<i32>,
    rating: Option<f64>,
) -> Vec<String> {
    let mut out: Vec<String> = genres
        .into_iter()
        .flatten()
        .map(|g| format!("genre:{}", g.to_lowercase()))
        .collect();
    if let Some(year) = year {
        out.push(format!("decade:{}", year / 10 * 10));
    }
    if let Some(rating) = rating.filter(|r| *r > 0.0) {
        let bucket = match rating {
            r if r < 6.0 => "<6",
            r if r < 7.0 => "6-7",
            r if r < 8.0 => "7-8",
            _ => "8+",
        };
        out.push(format!("rating:{bucket}"));
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct TasteModel {
    weights: HashMap<String, f64>,
    bias: f64,
    examples: usize,
}

impl TasteModel {
    /// Fits a logistic regression with class weights, so a history with few
    /// dropped shows still learns what they have in common.
    pub fn train(examples: &[(Vec<String>, bool)]) -> Self {
        let mut model = TasteModel {
            examples: examples.len(),
            ..Default::default()
        };
        let liked = examples.iter().filter(|(_, l)| *l).count();
        let disliked = examples.len() - liked;
        if liked == 0 || disliked == 0 {
            // One class only: nothing to tell apart. Score everything neutral.
            return model;
        }
        let weight_of = |l: bool| {
            let n = if l { liked } else { disliked };
            examples.len() as f64 / (2.0 * n as f64)
        };

        for _ in 0..EPOCHS {
            for (features, liked) in examples {
                let target = if *liked { 1.0 } else { 0.0 };
                let error = (model.predict(features) - target) * weight_of(*liked);
                model.bias -= LEARNING_RATE * error;
                for f in features {
                    let w = model.weights.entry(f.clone()).or_default();
                    *w -= LEARNING_RATE * (error + L2 * *w);
                }
            }
        }
        model
    }

    /// Probability (0–1) that the user likes a show with these features.
    pub fn predict(&self, features: &[String]) -> f64 {
        let z = self.bias
            + features
                .iter()
                .filter_map(|f| self.weights.get(f))
                .sum::<f64>();
        1.0 / (1.0 + (-z).exp())
    }

    pub fn score(&self, show: &Show) -> f64 {
        self.predict(&features(show))
    }

    pub fn score_movie(&self, movie: &Movie) -> f64 {
        self.predict(&movie_features(movie))
    }

    /// Enough history to trust the scores.
    pub fn is_useful(&self) -> bool {
        self.examples >= MIN_EXAMPLES && !self.weights.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn show(genres: &[&str], network: &str) -> Show {
        Show {
            genres: Some(genres.iter().map(|g| g.to_string()).collect()),
            network: Some(network.into()),
            ..Default::default()
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 12, 12, 0, 0).unwrap()
    }

    fn item(aired: u32, completed: u32, days_since_watch: i64) -> ProgressItem {
        ProgressItem {
            show: Show::default(),
            aired,
            completed,
            last_watched_at: Some(now() - Duration::days(days_since_watch)),
            next_episode: None,
            poster: None,
        }
    }

    #[test]
    fn labels_from_history() {
        assert_eq!(label(&item(20, 15, 5), None, now()), Some(true));
        assert_eq!(label(&item(100, 12, 5), None, now()), Some(true));
        // Abandoned after one episode, half a year ago.
        assert_eq!(label(&item(10, 1, 180), None, now()), Some(false));
        // Just started: unclear.
        assert_eq!(label(&item(10, 1, 2), None, now()), None);
        // Dropped or on hold is a clear no.
        assert_eq!(
            label(&item(20, 15, 5), Some(ShowStatus::Dropped), now()),
            Some(false)
        );
    }

    #[test]
    fn learns_genre_preferences() {
        let mut examples = Vec::new();
        for _ in 0..8 {
            examples.push((features(&show(&["drama", "crime"], "HBO")), true));
            examples.push((features(&show(&["reality"], "Bravo")), false));
        }
        let model = TasteModel::train(&examples);
        assert!(model.is_useful());
        let drama = model.score(&show(&["drama"], "AMC"));
        let reality = model.score(&show(&["reality"], "MTV"));
        assert!(drama > 0.6, "drama {drama}");
        assert!(reality < 0.4, "reality {reality}");
    }

    #[test]
    fn scores_stay_calibrated_with_lopsided_history() {
        // Mostly liked shows across genres, a few dropped: typical history.
        let genres = ["drama", "comedy", "animation", "crime", "fantasy"];
        let mut examples: Vec<_> = (0..80)
            .map(|i| (features(&show(&[genres[i % 5]], &format!("net{i}"))), true))
            .collect();
        examples.extend((0..10).map(|i| (features(&show(&["reality"], &format!("tv{i}"))), false)));
        let model = TasteModel::train(&examples);
        let drama = model.score(&show(&["drama"], "new network"));
        let reality = model.score(&show(&["reality"], "new network"));
        assert!(drama > reality);
        assert!(
            drama < 0.95,
            "unseen network drama should not be near-certain: {drama}"
        );
    }

    #[test]
    fn one_sided_history_scores_neutral() {
        let examples = vec![(features(&show(&["drama"], "HBO")), true); 12];
        let model = TasteModel::train(&examples);
        assert!(!model.is_useful());
        assert_eq!(model.score(&show(&["drama"], "HBO")), 0.5);
    }
}
