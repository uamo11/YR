use crate::models::{
    AudioQuality, GoogleAccountInfo, HomeFeed, HomeShelf, HomeShelfCard, HomeShelfLayout, LocalPlaylist,
    ShelfCardKind, Track,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHistoryItem {
    pub query: String,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqualizerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_preset_name")]
    pub preset_name: String,
    #[serde(default)]
    pub bass_boost_db: f32,
    #[serde(default = "default_eq_bands")]
    pub bands_db: [f32; 5],
    #[serde(default = "default_true")]
    pub loudness_normalization: bool,
    #[serde(default)]
    pub morphe_spatial_audio: bool,
    #[serde(default)]
    pub morphe_vocal_boost: bool,
    #[serde(default)]
    pub morphe_dynamic_bass: bool,
    #[serde(default = "default_true")]
    pub morphe_limiter: bool,
}

fn default_preset_name() -> String {
    "Plano".to_string()
}

fn default_eq_bands() -> [f32; 5] {
    [0.0, 0.0, 0.0, 0.0, 0.0]
}

impl Default for EqualizerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            preset_name: default_preset_name(),
            bass_boost_db: 0.0,
            bands_db: default_eq_bands(),
            loudness_normalization: true,
            morphe_spatial_audio: false,
            morphe_vocal_boost: false,
            morphe_dynamic_bass: false,
            morphe_limiter: true,
        }
    }
}

impl EqualizerConfig {
    pub const BAND_FREQS: [u32; 5] = [60, 230, 910, 3600, 14000];
    pub const BAND_LABELS: [&'static str; 5] =
        ["60 Hz", "230 Hz", "910 Hz", "3.6 kHz", "14 kHz"];

    pub fn presets() -> &'static [(&'static str, [f32; 5], f32)] {
        &[
            ("Plano", [0.0, 0.0, 0.0, 0.0, 0.0], 0.0),
            ("Graves +", [6.0, 4.0, 1.0, 0.0, -1.0], 6.0),
            ("Rock", [4.5, 2.5, -1.0, 2.5, 4.0], 2.0),
            ("Pop", [-1.0, 2.0, 3.5, 2.0, -1.0], 1.0),
            ("Electrónica", [5.0, 3.5, -1.0, 2.5, 4.5], 4.0),
            ("Vocal", [-2.0, 0.0, 4.0, 3.5, 1.0], 0.0),
            ("Acústico", [3.0, 2.0, 1.5, 2.5, 3.0], 0.0),
        ]
    }

    pub fn apply_preset(&mut self, name: &str) {
        if let Some((_, bands, bass)) = Self::presets().iter().find(|(n, _, _)| *n == name) {
            self.preset_name = name.to_string();
            self.bands_db = *bands;
            self.bass_boost_db = *bass;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub stream_quality: AudioQuality,
    pub download_quality: AudioQuality,
    pub max_concurrent_downloads: usize,
    pub volume: f32,
    #[serde(default = "default_one")]
    pub playback_speed: f32,
    #[serde(default = "default_true")]
    pub autoplay_radio: bool,
    #[serde(default = "default_true")]
    pub sponsorblock_enabled: bool,
    #[serde(default)]
    pub equalizer: EqualizerConfig,

    // Ahorro de datos y batería
    #[serde(default)]
    pub wifi_only_streaming: bool,
    #[serde(default = "default_true")]
    pub mobile_data_saver: bool,
    #[serde(default)]
    pub battery_saver: bool,
    #[serde(default)]
    pub prefer_video_mode: bool,

    // Descargas y almacenamiento
    #[serde(default = "default_true")]
    pub smart_downloads: bool,
    #[serde(default)]
    pub wifi_only_downloads: bool,
    #[serde(default = "default_max_stream_cache_mb")]
    pub max_stream_cache_mb: u32,
    #[serde(default = "default_max_smart_download_songs")]
    pub max_smart_download_songs: usize,
    #[serde(default = "default_max_smart_download_mb")]
    pub max_smart_download_mb: u32,

    // Notificaciones y privacidad
    #[serde(default = "default_true")]
    pub notifications_enabled: bool,
    #[serde(default)]
    pub incognito_mode: bool,
    #[serde(default = "default_true")]
    pub personalized_recommendations: bool,

    // Morphe Audio DSP
    #[serde(default)]
    pub morphe_spatial_audio: bool,
    #[serde(default)]
    pub morphe_vocal_boost: bool,
    #[serde(default)]
    pub morphe_dynamic_bass: bool,
    #[serde(default = "default_true")]
    pub morphe_limiter: bool,

    // Centro familiar
    #[serde(default)]
    pub restricted_mode: bool,

    // Comportamiento de reproducción y búsqueda
    #[serde(default = "default_seek_seconds")]
    pub seek_seconds: u32,
    #[serde(default = "default_true")]
    pub gapless_playback: bool,
}

fn default_seek_seconds() -> u32 {
    10
}

fn default_true() -> bool {
    true
}

fn default_one() -> f32 {
    1.0
}

fn default_max_stream_cache_mb() -> u32 {
    512
}

fn default_max_smart_download_songs() -> usize {
    100
}

fn default_max_smart_download_mb() -> u32 {
    1024
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            stream_quality: AudioQuality::High,
            download_quality: AudioQuality::High,
            max_concurrent_downloads: 4,
            volume: 0.85,
            playback_speed: 1.0,
            autoplay_radio: true,
            sponsorblock_enabled: true,
            equalizer: EqualizerConfig::default(),
            wifi_only_streaming: false,
            mobile_data_saver: true,
            battery_saver: false,
            prefer_video_mode: false,
            smart_downloads: true,
            wifi_only_downloads: false,
            max_stream_cache_mb: default_max_stream_cache_mb(),
            max_smart_download_songs: default_max_smart_download_songs(),
            max_smart_download_mb: default_max_smart_download_mb(),
            notifications_enabled: true,
            incognito_mode: false,
            personalized_recommendations: true,
            morphe_spatial_audio: false,
            morphe_vocal_boost: false,
            morphe_dynamic_bass: false,
            morphe_limiter: true,
            restricted_mode: false,
            seek_seconds: default_seek_seconds(),
            gapless_playback: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LibraryData {
    pub liked_tracks: Vec<Track>,
    pub recent_tracks: Vec<Track>,
    pub playlists: Vec<LocalPlaylist>,
    #[serde(default)]
    pub search_history: Vec<String>,
    #[serde(default)]
    pub search_history_items: Vec<SearchHistoryItem>,
    #[serde(default)]
    pub pinned_cards: Vec<HomeShelfCard>,
    #[serde(default)]
    pub disliked_ids: Vec<String>,
    #[serde(default)]
    pub last_queue: Vec<Track>,
    #[serde(default)]
    pub last_queue_index: Option<usize>,
    #[serde(default)]
    pub google_cookie: Option<String>,
    #[serde(default)]
    pub google_account: Option<GoogleAccountInfo>,
    pub config: AppConfig,
}

/// Atomic JSON-backed store for user library, history, and configuration.
#[derive(Debug, Clone)]
pub struct LibraryStore {
    path: PathBuf,
    pub data: LibraryData,
}

impl LibraryStore {
    pub fn load_or_default(base_dir: &Path) -> Self {
        let path = base_dir.join("library.json");
        let mut data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<LibraryData>(&raw).ok())
            .unwrap_or_default();
        data.recent_tracks
            .retain(|t| !t.id.starts_with("local_demo_"));
        Self { path, data }
    }

    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(&self.data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn is_liked(&self, track_id: &str) -> bool {
        self.data.liked_tracks.iter().any(|t| t.id == track_id)
    }

    pub fn toggle_like(&mut self, track: Track) -> bool {
        if let Some(pos) = self.data.liked_tracks.iter().position(|t| t.id == track.id) {
            self.data.liked_tracks.remove(pos);
            let _ = self.save();
            false
        } else {
            self.data.liked_tracks.insert(0, track);
            let _ = self.save();
            true
        }
    }

    pub fn record_play(&mut self, track: Track) {
        if track.id.starts_with("local_demo_") {
            return;
        }
        self.data.recent_tracks.retain(|t| t.id != track.id);
        self.data.recent_tracks.insert(0, track);
        if self.data.recent_tracks.len() > 50 {
            self.data.recent_tracks.truncate(50);
        }
        let _ = self.save();
    }

    pub fn create_playlist(&mut self, name: impl Into<String>) -> LocalPlaylist {
        let pl = LocalPlaylist {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            tracks: Vec::new(),
            created_at: Utc::now(),
        };
        self.data.playlists.push(pl.clone());
        let _ = self.save();
        pl
    }

    pub fn add_to_playlist(&mut self, playlist_id: &str, track: Track) {
        if let Some(pl) = self.data.playlists.iter_mut().find(|p| p.id == playlist_id) {
            if !pl.tracks.iter().any(|t| t.id == track.id) {
                pl.tracks.push(track);
                let _ = self.save();
            }
        }
    }

    pub fn remove_from_playlist(&mut self, playlist_id: &str, track_id: &str) {
        if let Some(pl) = self.data.playlists.iter_mut().find(|p| p.id == playlist_id) {
            let before = pl.tracks.len();
            pl.tracks.retain(|t| t.id != track_id);
            if pl.tracks.len() != before {
                let _ = self.save();
            }
        }
    }

    pub fn set_last_queue(&mut self, tracks: Vec<Track>, index: Option<usize>) {
        self.data.last_queue = tracks
            .into_iter()
            .filter(|t| !t.id.starts_with("local_demo_"))
            .take(50)
            .collect();
        self.data.last_queue_index = index.filter(|&i| i < self.data.last_queue.len());
    }

    pub fn delete_playlist(&mut self, playlist_id: &str) {
        self.data.playlists.retain(|p| p.id != playlist_id);
        let _ = self.save();
    }

    pub fn set_google_session(&mut self, cookie: Option<String>, account: Option<GoogleAccountInfo>) {
        self.data.google_cookie = cookie;
        self.data.google_account = account;
        let _ = self.save();
    }

    pub fn clear_google_session(&mut self) {
        self.data.google_cookie = None;
        self.data.google_account = None;
        let _ = self.save();
    }

    pub fn record_search(&mut self, query: &str) {
        self.record_search_with_thumb(query, None);
    }

    pub fn record_search_with_thumb(&mut self, query: &str, thumb: Option<String>) {
        let q = query.trim();
        if q.is_empty() {
            return;
        }
        self.data.search_history.retain(|s| !s.eq_ignore_ascii_case(q));
        self.data.search_history.insert(0, q.to_string());
        if self.data.search_history.len() > 15 {
            self.data.search_history.truncate(15);
        }

        let existing_thumb = thumb.or_else(|| {
            self.data
                .search_history_items
                .iter()
                .find(|i| i.query.eq_ignore_ascii_case(q))
                .and_then(|i| i.thumbnail_url.clone())
        });
        self.data
            .search_history_items
            .retain(|i| !i.query.eq_ignore_ascii_case(q));
        self.data.search_history_items.insert(
            0,
            SearchHistoryItem {
                query: q.to_string(),
                thumbnail_url: existing_thumb,
            },
        );
        if self.data.search_history_items.len() > 20 {
            self.data.search_history_items.truncate(20);
        }
        let _ = self.save();
    }

    pub fn remove_search_history(&mut self, query: &str) {
        self.data.search_history.retain(|s| s != query);
        self.data
            .search_history_items
            .retain(|i| !i.query.eq_ignore_ascii_case(query));
        let _ = self.save();
    }

    pub fn is_pinned_speed_dial(&self, title: &str) -> bool {
        self.data
            .pinned_cards
            .iter()
            .any(|c| c.title.eq_ignore_ascii_case(title))
    }

    pub fn toggle_pin_speed_dial(&mut self, card: HomeShelfCard) -> bool {
        if let Some(pos) = self
            .data
            .pinned_cards
            .iter()
            .position(|c| c.title.eq_ignore_ascii_case(&card.title))
        {
            self.data.pinned_cards.remove(pos);
            let _ = self.save();
            false
        } else {
            self.data.pinned_cards.insert(0, card);
            if self.data.pinned_cards.len() > 18 {
                self.data.pinned_cards.truncate(18);
            }
            let _ = self.save();
            true
        }
    }

    pub fn is_disliked(&self, track_id: &str) -> bool {
        self.data.disliked_ids.iter().any(|id| id == track_id)
    }

    pub fn toggle_dislike(&mut self, track_id: &str) -> bool {
        if let Some(pos) = self.data.disliked_ids.iter().position(|id| id == track_id) {
            self.data.disliked_ids.remove(pos);
            let _ = self.save();
            false
        } else {
            self.data.liked_tracks.retain(|t| t.id != track_id);
            self.data.disliked_ids.insert(0, track_id.to_string());
            if self.data.disliked_ids.len() > 200 {
                self.data.disliked_ids.truncate(200);
            }
            let _ = self.save();
            true
        }
    }

    pub fn has_taste_signals(&self) -> bool {
        !self.data.liked_tracks.is_empty()
            || !self.data.recent_tracks.is_empty()
            || self.data.playlists.iter().any(|p| !p.tracks.is_empty())
            || !self.data.search_history_items.is_empty()
            || !self.data.search_history.is_empty()
    }

    /// Excludes explicitly disliked tracks and reranks candidates using local likes,
    /// playlists, listening recency, and search queries with artist-diversity dampening.
    pub fn rank_recommendations(&self, tracks: Vec<Track>) -> Vec<Track> {
        let disliked: HashSet<&str> = self.data.disliked_ids.iter().map(String::as_str).collect();
        let mut seen_ids = HashSet::new();
        let filtered: Vec<Track> = tracks
            .into_iter()
            .filter(|t| !disliked.contains(t.id.as_str()) && seen_ids.insert(t.id.clone()))
            .collect();

        if filtered.len() <= 1 || !self.has_taste_signals() {
            return filtered;
        }

        let mut artist_weights: HashMap<String, f32> = HashMap::new();
        let mut token_weights: HashMap<String, f32> = HashMap::new();

        let mut add_weight = |artist: &str, text: &str, a_w: f32, t_w: f32| {
            let norm_artist = normalize_key(artist);
            if !norm_artist.is_empty() && norm_artist != "youtube music" {
                *artist_weights.entry(norm_artist).or_default() += a_w;
            }
            for tok in taste_tokens(text) {
                *token_weights.entry(tok).or_default() += t_w;
            }
        };

        for (idx, t) in self.data.liked_tracks.iter().take(40).enumerate() {
            let decay = 1.0 / (1.0 + idx as f32 * 0.05);
            add_weight(&t.artist, &t.title, 4.0 * decay, 1.4 * decay);
        }
        for pl in &self.data.playlists {
            for t in pl.tracks.iter().take(30) {
                add_weight(&t.artist, &t.title, 2.8, 1.0);
            }
        }
        for (idx, t) in self.data.recent_tracks.iter().take(40).enumerate() {
            let decay = 1.0 / (1.0 + idx as f32 * 0.14);
            add_weight(&t.artist, &t.title, 2.4 * decay, 0.8 * decay);
        }
        for item in self.data.search_history_items.iter().take(15) {
            for tok in taste_tokens(&item.query) {
                *token_weights.entry(tok).or_default() += 1.5;
            }
        }
        for q in self.data.search_history.iter().take(15) {
            for tok in taste_tokens(q) {
                *token_weights.entry(tok).or_default() += 1.2;
            }
        }

        let n = filtered.len().max(1) as f32;
        let mut scored: Vec<(usize, String, f32, Track)> = filtered
            .into_iter()
            .enumerate()
            .map(|(orig_idx, track)| {
                let norm_artist = normalize_key(&track.artist);
                let artist_bonus = artist_weights.get(&norm_artist).copied().unwrap_or(0.0);
                let tok_bonus: f32 = taste_tokens(&format!("{} {}", track.title, track.artist))
                    .into_iter()
                    .filter_map(|tok| token_weights.get(&tok).copied())
                    .sum::<f32>()
                    .min(4.0);
                // Keep subtle baseline preference for upstream ranking so fresh discovery stays crisp.
                let prior = (1.0 - (orig_idx as f32 / n)) * 1.2;
                (orig_idx, norm_artist, prior + artist_bonus + tok_bonus, track)
            })
            .collect();

        // Greedy selection applying per-artist dampening so no single artist dominates consecutive slots.
        let mut out = Vec::with_capacity(scored.len());
        let mut artist_counts: HashMap<String, usize> = HashMap::new();
        while !scored.is_empty() {
            let mut best_pos = 0usize;
            let mut best_eff = f32::NEG_INFINITY;
            for (pos, (orig_idx, norm_artist, raw_score, _)) in scored.iter().enumerate() {
                let repeats = artist_counts.get(norm_artist).copied().unwrap_or(0) as f32;
                let eff = *raw_score - repeats * 2.2 - (*orig_idx as f32 * 0.001);
                if eff > best_eff {
                    best_eff = eff;
                    best_pos = pos;
                }
            }
            let (_, norm_artist, _, track) = scored.remove(best_pos);
            if !norm_artist.is_empty() {
                *artist_counts.entry(norm_artist).or_default() += 1;
            }
            out.push(track);
        }
        out
    }

    /// Suppresses disliked items from the home feed and enriches recommendations using local taste.
    pub fn personalize_home_feed(&self, feed: &mut HomeFeed) {
        let disliked: HashSet<&str> = self.data.disliked_ids.iter().map(String::as_str).collect();
        if !disliked.is_empty() {
            feed.quick_picks.retain(|t| !disliked.contains(t.id.as_str()));
            feed.recommended_tracks
                .retain(|t| !disliked.contains(t.id.as_str()));
            feed.listen_again.retain(|t| !disliked.contains(t.id.as_str()));
            feed.speed_dial.retain(|c| match &c.kind {
                ShelfCardKind::Track(t) => !disliked.contains(t.id.as_str()),
                _ => true,
            });
            for shelf in &mut feed.shelves {
                shelf.tracks.retain(|t| !disliked.contains(t.id.as_str()));
                shelf.cards.retain(|c| match &c.kind {
                    ShelfCardKind::Track(t) => !disliked.contains(t.id.as_str()),
                    _ => true,
                });
            }
        }

        if !self.has_taste_signals() {
            return;
        }

        feed.recommended_tracks =
            self.rank_recommendations(std::mem::take(&mut feed.recommended_tracks));

        // Gather broad discovery pool excluding disliked tracks to build a local affinity shelf when needed.
        let mut pool = Vec::new();
        pool.extend(feed.recommended_tracks.iter().cloned());
        pool.extend(feed.quick_picks.iter().cloned());
        for s in &feed.shelves {
            pool.extend(s.tracks.iter().cloned());
        }
        pool.extend(
            self.data
                .liked_tracks
                .iter()
                .chain(self.data.recent_tracks.iter())
                .filter(|t| !t.id.starts_with("local_demo_"))
                .cloned(),
        );
        let personalized = self.rank_recommendations(pool);

        let already_has_personal_shelf = feed.shelves.iter().any(|s| {
            let l = s.title.to_lowercase();
            l.contains("tus gustos") || l.contains("para ti")
        });
        if !already_has_personal_shelf && personalized.len() >= 4 {
            let shelf_tracks: Vec<Track> = personalized.into_iter().take(16).collect();
            let shelf_cards: Vec<HomeShelfCard> = shelf_tracks
                .iter()
                .map(|t| HomeShelfCard {
                    title: t.title.clone(),
                    subtitle: t.artist.clone(),
                    thumbnail_url: t.thumbnail_url.clone(),
                    kind: ShelfCardKind::Track(t.clone()),
                })
                .collect();
            let insert_at = feed
                .shelves
                .iter()
                .position(|s| s.layout == HomeShelfLayout::SpeedDialGrid)
                .map(|p| p + 1)
                .unwrap_or(1)
                .min(feed.shelves.len());
            feed.shelves.insert(
                insert_at,
                HomeShelf {
                    title: "Basado en tus gustos".to_string(),
                    eyebrow: Some("RECOMENDADO PARA TI".to_string()),
                    layout: HomeShelfLayout::QuickPicks4Row,
                    tracks: shelf_tracks,
                    cards: shelf_cards,
                },
            );
        }
    }

    /// Searches liked tracks, playlists, and recent history offline with zero latency.
    pub fn search_local(&self, query: &str) -> Vec<Track> {
        let q = normalize_key(query);
        if q.len() < 2 {
            return Vec::new();
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let candidates = self
            .data
            .liked_tracks
            .iter()
            .chain(self.data.playlists.iter().flat_map(|p| p.tracks.iter()))
            .chain(self.data.recent_tracks.iter());

        for t in candidates {
            if self.is_disliked(&t.id) || !seen.insert(t.id.clone()) {
                continue;
            }
            let title_m = t.title.to_lowercase().contains(&q);
            let artist_m = t.artist.to_lowercase().contains(&q);
            let album_m = t
                .album
                .as_deref()
                .map(|a| a.to_lowercase().contains(&q))
                .unwrap_or(false);
            if title_m || artist_m || album_m {
                out.push(t.clone());
                if out.len() >= 12 {
                    break;
                }
            }
        }
        out
    }
}

fn normalize_key(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn taste_tokens(s: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "the", "and", "for", "with", "feat", "official", "audio", "video", "live", "remix",
        "en", "vivo", "con", "los", "las", "del", "para", "por", "una",
    ];
    s.split(|c: char| !c.is_alphanumeric())
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() >= 3 && !STOPWORDS.contains(&w.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_persists_likes_and_recent() {
        let dir = std::env::temp_dir().join(format!("yr_store_test_{}", Uuid::new_v4()));
        let mut store = LibraryStore::load_or_default(&dir);
        let track = Track {
            id: "abc".into(),
            title: "Test".into(),
            artist: "Artist".into(),
            artist_id: None,
            album: None,
            album_id: None,
            duration_secs: Some(120),
            thumbnail_url: None,
            local_path: None,
        };
        assert!(store.toggle_like(track.clone()));
        store.record_play(track);

        let reloaded = LibraryStore::load_or_default(&dir);
        assert!(reloaded.is_liked("abc"));
        assert_eq!(reloaded.data.recent_tracks.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn make_track(id: &str, title: &str, artist: &str) -> Track {
        Track {
            id: id.into(),
            title: title.into(),
            artist: artist.into(),
            artist_id: None,
            album: None,
            album_id: None,
            duration_secs: Some(180),
            thumbnail_url: None,
            local_path: None,
        }
    }

    #[test]
    fn rank_recommendations_excludes_disliked_and_boosts_liked_artists() {
        let dir = std::env::temp_dir().join(format!("yr_rank_test_{}", Uuid::new_v4()));
        let mut store = LibraryStore::load_or_default(&dir);
        store.toggle_like(make_track("like1", "Believer", "Imagine Dragons"));
        store.toggle_dislike("bad1");

        let ranked = store.rank_recommendations(vec![
            make_track("bad1", "Terrible Song", "Imagine Dragons"),
            make_track("other1", "Random Track", "Unknown Band"),
            make_track("good1", "Thunder", "Imagine Dragons"),
        ]);

        assert_eq!(ranked.len(), 2);
        assert!(ranked.iter().all(|t| t.id != "bad1"));
        assert_eq!(ranked[0].id, "good1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rank_recommendations_applies_artist_diversity_dampening() {
        let dir = std::env::temp_dir().join(format!("yr_div_test_{}", Uuid::new_v4()));
        let mut store = LibraryStore::load_or_default(&dir);
        store.toggle_like(make_track("like_a", "Track A", "Artist A"));
        store.record_play(make_track("play_b", "Track B", "Artist B"));

        let ranked = store.rank_recommendations(vec![
            make_track("a1", "One", "Artist A"),
            make_track("a2", "Two", "Artist A"),
            make_track("a3", "Three", "Artist A"),
            make_track("b1", "Four", "Artist B"),
        ]);

        assert_eq!(ranked[0].artist, "Artist A");
        // After the first Artist A pick, dampening lets Artist B appear before exhausting all Artist A items.
        assert!(ranked[1].artist == "Artist B" || ranked[2].artist == "Artist B");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn personalize_home_feed_filters_dislikes_and_injects_taste_shelf() {
        let dir = std::env::temp_dir().join(format!("yr_home_test_{}", Uuid::new_v4()));
        let mut store = LibraryStore::load_or_default(&dir);
        store.toggle_like(make_track("liked1", "Favorite Hit", "Fav Artist"));
        store.toggle_dislike("hate1");

        let mut feed = HomeFeed {
            quick_picks: vec![
                make_track("hate1", "Hated", "Other"),
                make_track("q1", "Q1", "Other"),
                make_track("q2", "Q2", "Fav Artist"),
                make_track("q3", "Q3", "Someone"),
            ],
            ..Default::default()
        };
        store.personalize_home_feed(&mut feed);

        assert!(feed.quick_picks.iter().all(|t| t.id != "hate1"));
        assert!(feed.shelves.iter().any(|s| s.title == "Basado en tus gustos"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_local_matches_liked_playlists_and_recent_offline() {
        let dir = std::env::temp_dir().join(format!("yr_local_search_{}", Uuid::new_v4()));
        let mut store = LibraryStore::load_or_default(&dir);
        store.toggle_like(make_track("b1", "Believer", "Imagine Dragons"));
        store.record_play(make_track("d1", "Demons", "Imagine Dragons"));
        store.toggle_dislike("d1");

        let hits = store.search_local("imagine");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "b1");
        assert!(store.search_local("a").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn queue_speed_and_playlist_remove_persist() {
        let dir = std::env::temp_dir().join(format!("yr_persist_q_{}", Uuid::new_v4()));
        let mut store = LibraryStore::load_or_default(&dir);
        let pl = store.create_playlist("Gym");
        let t1 = make_track("t1", "Track One", "Artist 1");
        let t2 = make_track("t2", "Track Two", "Artist 2");
        store.add_to_playlist(&pl.id, t1.clone());
        store.add_to_playlist(&pl.id, t2.clone());
        store.remove_from_playlist(&pl.id, "t1");
        store.data.config.playback_speed = 1.25;
        store.set_last_queue(vec![t1, t2], Some(1));
        store.save().unwrap();

        let reloaded = LibraryStore::load_or_default(&dir);
        assert_eq!(reloaded.data.playlists[0].tracks.len(), 1);
        assert_eq!(reloaded.data.playlists[0].tracks[0].id, "t2");
        assert!((reloaded.data.config.playback_speed - 1.25).abs() < 0.001);
        assert_eq!(reloaded.data.last_queue.len(), 2);
        assert_eq!(reloaded.data.last_queue_index, Some(1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn equalizer_config_presets_and_persistence() {
        let dir = std::env::temp_dir().join(format!("yr_eq_test_{}", Uuid::new_v4()));
        let mut store = LibraryStore::load_or_default(&dir);
        assert!(!store.data.config.equalizer.enabled);
        assert_eq!(store.data.config.equalizer.preset_name, "Plano");

        store.data.config.equalizer.enabled = true;
        store.data.config.equalizer.apply_preset("Rock");
        assert_eq!(store.data.config.equalizer.preset_name, "Rock");
        assert_eq!(store.data.config.equalizer.bands_db[0], 4.5);
        assert_eq!(store.data.config.equalizer.bass_boost_db, 2.0);
        store.save().unwrap();

        let reloaded = LibraryStore::load_or_default(&dir);
        assert!(reloaded.data.config.equalizer.enabled);
        assert_eq!(reloaded.data.config.equalizer.preset_name, "Rock");
        assert_eq!(reloaded.data.config.equalizer.bands_db[0], 4.5);
        assert_eq!(reloaded.data.config.equalizer.bass_boost_db, 2.0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
