use crate::models::{AudioQuality, LocalPlaylist, Track};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub stream_quality: AudioQuality,
    pub download_quality: AudioQuality,
    pub max_concurrent_downloads: usize,
    pub volume: f32,
    #[serde(default = "default_true")]
    pub autoplay_radio: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            stream_quality: AudioQuality::High,
            download_quality: AudioQuality::High,
            max_concurrent_downloads: 3,
            volume: 0.85,
            autoplay_radio: true,
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
        let data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<LibraryData>(&raw).ok())
            .unwrap_or_default();
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

    pub fn delete_playlist(&mut self, playlist_id: &str) {
        self.data.playlists.retain(|p| p.id != playlist_id);
        let _ = self.save();
    }

    pub fn record_search(&mut self, query: &str) {
        let q = query.trim();
        if q.is_empty() {
            return;
        }
        self.data.search_history.retain(|s| !s.eq_ignore_ascii_case(q));
        self.data.search_history.insert(0, q.to_string());
        if self.data.search_history.len() > 15 {
            self.data.search_history.truncate(15);
        }
        let _ = self.save();
    }

    pub fn remove_search_history(&mut self, query: &str) {
        self.data.search_history.retain(|s| s != query);
        let _ = self.save();
    }
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
}
