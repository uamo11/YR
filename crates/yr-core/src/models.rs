use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Preferred audio quality for streaming and downloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AudioQuality {
    Low,
    Medium,
    #[default]
    High,
    Best,
}

impl AudioQuality {
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Baja (~64 kbps)",
            Self::Medium => "Media (~128 kbps)",
            Self::High => "Alta (~160 kbps)",
            Self::Best => "Máxima",
        }
    }

    pub fn target_bitrate_kbps(self) -> u32 {
        match self {
            Self::Low => 64,
            Self::Medium => 128,
            Self::High => 160,
            Self::Best => 320,
        }
    }
}

/// Core track metadata used across UI, queue, audio, and download subsystems.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub artist_id: Option<String>,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub duration_secs: Option<u64>,
    pub thumbnail_url: Option<String>,
    /// Local file path if the track has been downloaded offline.
    pub local_path: Option<String>,
}

impl Track {
    pub fn formatted_duration(&self) -> String {
        match self.duration_secs {
            Some(secs) => format_seconds(secs),
            None => "--:--".to_string(),
        }
    }
}

/// Format seconds into `MM:SS` or `H:MM:SS`.
pub fn format_seconds(total_secs: u64) -> String {
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hours > 0 {
        format!("{hours}:{mins:02}:{secs:02}")
    } else {
        format!("{mins}:{secs:02}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumSummary {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub year: Option<u16>,
    pub thumbnail_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistSummary {
    pub id: String,
    pub name: String,
    pub subscribers: Option<String>,
    pub thumbnail_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistSummary {
    pub id: String,
    pub title: String,
    pub author: Option<String>,
    pub track_count: Option<u32>,
    pub thumbnail_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchResults {
    pub songs: Vec<Track>,
    pub albums: Vec<AlbumSummary>,
    pub artists: Vec<ArtistSummary>,
    pub playlists: Vec<PlaylistSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumDetail {
    pub summary: AlbumSummary,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistDetail {
    pub summary: PlaylistSummary,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistDetail {
    pub summary: ArtistSummary,
    pub top_tracks: Vec<Track>,
    pub albums: Vec<AlbumSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoodGenre {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub params: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoogleDeviceAuth {
    pub user_code: String,
    pub verification_url: String,
    pub interval_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoogleAccountInfo {
    pub name: String,
    pub handle: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShelfCardKind {
    Track(Track),
    Album(AlbumSummary),
    Playlist(PlaylistSummary),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeShelfCard {
    pub title: String,
    pub subtitle: String,
    pub thumbnail_url: Option<String>,
    pub kind: ShelfCardKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HomeShelfLayout {
    QuickPicks4Row,
    CarouselCards,
    SpeedDialGrid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeShelf {
    pub title: String,
    pub eyebrow: Option<String>,
    pub layout: HomeShelfLayout,
    pub tracks: Vec<Track>,
    pub cards: Vec<HomeShelfCard>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HomeFeed {
    pub quick_picks: Vec<Track>,
    #[serde(default)]
    pub listen_again: Vec<Track>,
    #[serde(default)]
    pub recommended_tracks: Vec<Track>,
    #[serde(default)]
    pub trending_tracks: Vec<Track>,
    pub new_albums: Vec<AlbumSummary>,
    #[serde(default)]
    pub user_playlists: Vec<PlaylistSummary>,
    #[serde(default)]
    pub recommended_playlists: Vec<PlaylistSummary>,
    pub moods: Vec<MoodGenre>,
    #[serde(default)]
    pub is_logged_in: bool,
    #[serde(default)]
    pub account: Option<GoogleAccountInfo>,
    #[serde(default)]
    pub shelves: Vec<HomeShelf>,
    #[serde(default)]
    pub speed_dial: Vec<HomeShelfCard>,
}

/// Resolved direct audio stream metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamInfo {
    pub track_id: String,
    pub url: String,
    pub mime_type: String,
    pub extension: String,
    pub bitrate_bps: u32,
    pub content_length: Option<u64>,
}

/// Resolved direct video stream metadata for native in-place video playback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoStreamInfo {
    pub track_id: String,
    pub url: String,
    pub fallback_url: Option<String>,
    pub user_agent: String,
    pub width: u32,
    pub height: u32,
    pub is_hls: bool,
}

/// Persisted user playlist created locally inside YR.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalPlaylist {
    pub id: String,
    pub name: String,
    pub tracks: Vec<Track>,
    pub created_at: DateTime<Utc>,
}

/// Playback status reported by the audio engine.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PlaybackStatus {
    #[default]
    Idle,
    Loading,
    Playing,
    Paused,
    Buffering,
    Error(String),
}

/// Single timestamped line for synchronized lyrics (LRC / LRCLIB).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricLine {
    pub time_secs: f64,
    pub text: String,
}

/// Resolved lyrics payload supporting both plain text and synced LRC lines.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LyricsData {
    pub plain: String,
    #[serde(default)]
    pub synced: Vec<LyricLine>,
    #[serde(default)]
    pub source: String,
}

/// SponsorBlock non-music / intro / sponsor segment interval in seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SponsorSegment {
    pub start_secs: f64,
    pub end_secs: f64,
    pub category: String,
}

/// Explore feed sections (New Releases, Charts/Rankings, Moods & Genres).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExploreFeed {
    pub new_albums: Vec<AlbumSummary>,
    pub trending_tracks: Vec<Track>,
    pub moods: Vec<MoodGenre>,
}

/// Loads a JSON-serialized feed or snapshot from disk, returning `None` on missing or corrupt data.
pub fn load_json_cache<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Option<T> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Persists a feed snapshot atomically via a temporary sibling file so crashes never truncate cache.
pub fn save_json_cache<T: serde::Serialize>(path: &std::path::Path, value: &T) {
    let Ok(json) = serde_json::to_string(value) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_cache_roundtrip_home_and_explore_feeds() {
        let dir = std::env::temp_dir().join(format!("yr_cache_test_{}", uuid::Uuid::new_v4()));
        let home_path = dir.join("home_cache.json");
        let explore_path = dir.join("explore_cache.json");

        let sample_track = |id: &str, title: &str| Track {
            id: id.to_string(),
            title: title.to_string(),
            artist: "Imagine Dragons".to_string(),
            artist_id: None,
            album: None,
            album_id: None,
            duration_secs: Some(204),
            thumbnail_url: None,
            local_path: None,
        };

        let home = HomeFeed {
            shelves: vec![HomeShelf {
                title: "Selección rápida".to_string(),
                eyebrow: Some("PARA TI".to_string()),
                layout: HomeShelfLayout::QuickPicks4Row,
                tracks: vec![sample_track("7wtfhZwyrcc", "Believer")],
                cards: vec![HomeShelfCard {
                    title: "Believer".to_string(),
                    subtitle: "Imagine Dragons".to_string(),
                    thumbnail_url: None,
                    kind: ShelfCardKind::Track(sample_track("7wtfhZwyrcc", "Believer")),
                }],
            }],
            ..Default::default()
        };
        save_json_cache(&home_path, &home);
        let loaded_home: HomeFeed = load_json_cache(&home_path).expect("cached home feed");
        assert_eq!(loaded_home.shelves.len(), 1);
        assert_eq!(loaded_home.shelves[0].cards[0].title, "Believer");

        let explore = ExploreFeed {
            trending_tracks: vec![sample_track("t1", "Natural")],
            ..Default::default()
        };
        save_json_cache(&explore_path, &explore);
        let loaded_explore: ExploreFeed =
            load_json_cache(&explore_path).expect("cached explore feed");
        assert_eq!(loaded_explore.trending_tracks.len(), 1);
        assert_eq!(loaded_explore.trending_tracks[0].title, "Natural");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
