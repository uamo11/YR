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
