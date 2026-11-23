use std::path::{Path, PathBuf};

use crate::models::LyricsData;

/// Returns the deterministic local JSON cache file path for a track's lyrics.
pub fn offline_lyrics_path(lyrics_dir: &Path, track_id: &str) -> PathBuf {
    let sanitized: String = track_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let filename = if sanitized.is_empty() {
        "track.json".to_string()
    } else {
        format!("{sanitized}.json")
    };
    lyrics_dir.join(filename)
}

/// Returns true when `data` contains real lyrics worth persisting offline.
pub fn is_cacheable_lyrics(data: &LyricsData) -> bool {
    if !data.synced.is_empty() {
        return true;
    }
    let p = data.plain.trim();
    !p.is_empty() && !p.starts_with("No se pudo") && !p.starts_with("Letra no disponible")
}

/// Loads cached offline lyrics for `track_id` if present and valid.
pub fn load_cached_lyrics(lyrics_dir: &Path, track_id: &str) -> Option<LyricsData> {
    let path = offline_lyrics_path(lyrics_dir, track_id);
    let raw = std::fs::read_to_string(path).ok()?;
    let parsed: LyricsData = serde_json::from_str(&raw).ok()?;
    if is_cacheable_lyrics(&parsed) {
        Some(parsed)
    } else {
        None
    }
}

/// Atomically persists valid lyrics JSON to disk for instant offline playback.
pub fn save_cached_lyrics(lyrics_dir: &Path, track_id: &str, data: &LyricsData) -> std::io::Result<()> {
    if !is_cacheable_lyrics(data) {
        return Ok(());
    }
    std::fs::create_dir_all(lyrics_dir)?;
    let dst = offline_lyrics_path(lyrics_dir, track_id);
    let tmp = dst.with_extension("json.tmp");
    let json = serde_json::to_string(data)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &dst)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::LyricLine;

    fn temp_test_dir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        p.push(format!("yr_lyrics_cache_{tag}_{pid}_{nanos}"));
        p
    }

    #[test]
    fn roundtrip_synced_lyrics_and_sanitizes_id() {
        let dir = temp_test_dir("roundtrip");
        let data = LyricsData {
            plain: String::new(),
            synced: vec![
                LyricLine {
                    time_secs: 12.5,
                    text: "Primera línea".to_string(),
                },
                LyricLine {
                    time_secs: 18.0,
                    text: "Segunda línea".to_string(),
                },
            ],
            source: "LRCLIB".to_string(),
        };

        save_cached_lyrics(&dir, "../evil/track_id-123?", &data).expect("save succeeds");
        let path = offline_lyrics_path(&dir, "../evil/track_id-123?");
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("eviltrack_id-123.json")
        );
        let loaded = load_cached_lyrics(&dir, "../evil/track_id-123?").expect("loaded lyrics");
        assert_eq!(loaded, data);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ignores_empty_and_error_lyrics() {
        let dir = temp_test_dir("ignore");
        let empty = LyricsData::default();
        let err_msg = LyricsData {
            plain: "No se pudo cargar la letra: sin conexión".to_string(),
            synced: Vec::new(),
            source: String::new(),
        };
        let unavailable = LyricsData {
            plain: "Letra no disponible para esta canción.".to_string(),
            synced: Vec::new(),
            source: String::new(),
        };

        assert!(!is_cacheable_lyrics(&empty));
        assert!(!is_cacheable_lyrics(&err_msg));
        assert!(!is_cacheable_lyrics(&unavailable));

        save_cached_lyrics(&dir, "vid1", &empty).expect("ok");
        save_cached_lyrics(&dir, "vid2", &err_msg).expect("ok");
        save_cached_lyrics(&dir, "vid3", &unavailable).expect("ok");

        assert!(load_cached_lyrics(&dir, "vid1").is_none());
        assert!(load_cached_lyrics(&dir, "vid2").is_none());
        assert!(load_cached_lyrics(&dir, "vid3").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
