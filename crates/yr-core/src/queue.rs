use crate::models::Track;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

impl RepeatMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Sin repetir",
            Self::All => "Repetir todo",
            Self::One => "Repetir una",
        }
    }
}

/// Manages the active playback queue, shuffle order, and repeat behavior.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlaybackQueue {
    tracks: Vec<Track>,
    order: Vec<usize>,
    current_pos: Option<usize>,
    pub shuffle: bool,
    pub repeat: RepeatMode,
}

impl PlaybackQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    /// Clears all tracks and resets playback position.
    pub fn clear(&mut self) {
        self.tracks.clear();
        self.order.clear();
        self.current_pos = None;
    }

    pub fn current_track(&self) -> Option<&Track> {
        let pos = self.current_pos?;
        let idx = *self.order.get(pos)?;
        self.tracks.get(idx)
    }

    pub fn current_index(&self) -> Option<usize> {
        let pos = self.current_pos?;
        self.order.get(pos).copied()
    }

    /// Replace the entire queue and start at `start_index`.
    pub fn set_queue(&mut self, tracks: Vec<Track>, start_index: usize) {
        self.tracks = tracks;
        if self.tracks.is_empty() {
            self.order.clear();
            self.current_pos = None;
            return;
        }

        let clamped = start_index.min(self.tracks.len() - 1);
        self.rebuild_order(Some(clamped));
    }

    /// Append a track to the end of the queue.
    pub fn enqueue(&mut self, track: Track) {
        let new_idx = self.tracks.len();
        self.tracks.push(track);
        self.order.push(new_idx);
        if self.current_pos.is_none() {
            self.current_pos = Some(0);
        }
    }

    /// Backfill duration/thumbnail on an existing track or append if not yet in the queue.
    pub fn enqueue_or_enrich(&mut self, track: Track) {
        if let Some(existing) = self.tracks.iter_mut().find(|t| t.id == track.id) {
            if existing.duration_secs.unwrap_or(0) == 0 && track.duration_secs.is_some() {
                existing.duration_secs = track.duration_secs;
            }
            if existing.thumbnail_url.is_none() && track.thumbnail_url.is_some() {
                existing.thumbnail_url = track.thumbnail_url;
            }
            return;
        }
        self.enqueue(track);
    }

    /// Fill `duration_secs` on the matching track if not already known.
    pub fn fill_track_duration(&mut self, track_id: &str, duration_secs: u64) {
        if duration_secs == 0 {
            return;
        }
        for t in &mut self.tracks {
            if t.id == track_id && t.duration_secs.unwrap_or(0) == 0 {
                t.duration_secs = Some(duration_secs);
            }
        }
    }

    /// Insert a track immediately after the currently playing track.
    pub fn enqueue_next(&mut self, track: Track) {
        let new_idx = self.tracks.len();
        self.tracks.push(track);
        match self.current_pos {
            Some(pos) if pos + 1 <= self.order.len() => {
                self.order.insert(pos + 1, new_idx);
            }
            _ => {
                self.order.push(new_idx);
                self.current_pos = Some(0);
            }
        }
    }

    pub fn remove_at(&mut self, track_index: usize) {
        if track_index >= self.tracks.len() {
            return;
        }
        let current_track_idx = self.current_index();
        self.tracks.remove(track_index);

        // Rebuild order mapping indices > track_index down by 1.
        self.order.retain(|&idx| idx != track_index);
        for idx in &mut self.order {
            if *idx > track_index {
                *idx -= 1;
            }
        }

        if self.tracks.is_empty() {
            self.current_pos = None;
            return;
        }

        self.current_pos = match current_track_idx {
            Some(prev) if prev == track_index => {
                Some(self.current_pos.unwrap_or(0).min(self.order.len() - 1))
            }
            Some(prev) => {
                let target = if prev > track_index { prev - 1 } else { prev };
                self.order.iter().position(|&idx| idx == target)
            }
            None => None,
        };
    }

    /// Move an item up (-1) or down (+1) in the playback order while preserving `current_pos`.
    pub fn move_in_order(&mut self, order_pos: usize, delta: isize) -> bool {
        let target = order_pos as isize + delta;
        if target < 0 || order_pos >= self.order.len() || (target as usize) >= self.order.len() {
            return false;
        }
        let target = target as usize;
        self.order.swap(order_pos, target);
        if let Some(curr) = self.current_pos {
            if curr == order_pos {
                self.current_pos = Some(target);
            } else if curr == target {
                self.current_pos = Some(order_pos);
            }
        }
        true
    }

    pub fn jump_to(&mut self, track_index: usize) -> Option<&Track> {
        if track_index >= self.tracks.len() {
            return None;
        }
        let pos = self.order.iter().position(|&idx| idx == track_index)?;
        self.current_pos = Some(pos);
        self.current_track()
    }

    pub fn toggle_shuffle(&mut self) {
        let current_track_idx = self.current_index();
        self.shuffle = !self.shuffle;
        self.rebuild_order(current_track_idx);
    }

    pub fn next_track(&mut self, manual: bool) -> Option<&Track> {
        if self.tracks.is_empty() {
            return None;
        }
        if !manual && self.repeat == RepeatMode::One {
            return self.current_track();
        }

        let pos = self.current_pos.unwrap_or(0);
        if pos + 1 < self.order.len() {
            self.current_pos = Some(pos + 1);
            self.current_track()
        } else if self.repeat == RepeatMode::All || (manual && self.repeat == RepeatMode::One) {
            self.current_pos = Some(0);
            self.current_track()
        } else {
            None
        }
    }

    pub fn prev_track(&mut self) -> Option<&Track> {
        if self.tracks.is_empty() {
            return None;
        }
        let pos = self.current_pos.unwrap_or(0);
        if pos > 0 {
            self.current_pos = Some(pos - 1);
        } else if self.repeat == RepeatMode::All {
            self.current_pos = Some(self.order.len() - 1);
        } else {
            self.current_pos = Some(0);
        }
        self.current_track()
    }

    pub fn ordered_tracks(&self) -> Vec<(usize, &Track)> {
        self.order
            .iter()
            .filter_map(|&idx| self.tracks.get(idx).map(|t| (idx, t)))
            .collect()
    }

    fn rebuild_order(&mut self, keep_track_idx: Option<usize>) {
        self.order = (0..self.tracks.len()).collect();
        if self.tracks.is_empty() {
            self.current_pos = None;
            return;
        }

        if self.shuffle && self.order.len() > 1 {
            // Deterministic-free fast Fisher-Yates using timestamp nanos as seed.
            let mut seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x9E37_79B9_7F4A_7C15);

            for i in (1..self.order.len()).rev() {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let j = (seed as usize) % (i + 1);
                self.order.swap(i, j);
            }

            if let Some(target) = keep_track_idx {
                if let Some(pos) = self.order.iter().position(|&idx| idx == target) {
                    self.order.swap(0, pos);
                }
                self.current_pos = Some(0);
                return;
            }
        }

        self.current_pos = keep_track_idx.and_then(|t| self.order.iter().position(|&idx| idx == t));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_track(id: &str) -> Track {
        Track {
            id: id.to_string(),
            title: format!("Song {id}"),
            artist: "Artist".to_string(),
            artist_id: None,
            album: None,
            album_id: None,
            duration_secs: Some(180),
            thumbnail_url: None,
            local_path: None,
        }
    }

    #[test]
    fn queue_navigation_and_repeat() {
        let mut q = PlaybackQueue::new();
        q.set_queue(vec![sample_track("1"), sample_track("2"), sample_track("3")], 0);
        assert_eq!(q.current_track().unwrap().id, "1");
        assert_eq!(q.next_track(false).unwrap().id, "2");
        assert_eq!(q.next_track(false).unwrap().id, "3");
        assert!(q.next_track(false).is_none());

        q.repeat = RepeatMode::All;
        assert_eq!(q.next_track(false).unwrap().id, "1");
        assert_eq!(q.prev_track().unwrap().id, "3");

        q.repeat = RepeatMode::One;
        assert_eq!(q.next_track(false).unwrap().id, "3");
    }

    #[test]
    fn enqueue_next_and_remove() {
        let mut q = PlaybackQueue::new();
        q.set_queue(vec![sample_track("1"), sample_track("2")], 0);
        q.enqueue_next(sample_track("X"));
        assert_eq!(q.next_track(true).unwrap().id, "X");
        q.remove_at(1);
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn enqueue_or_enrich_dedupes_and_fills_duration() {
        let mut q = PlaybackQueue::new();
        let mut bare = sample_track("1");
        bare.duration_secs = None;
        q.set_queue(vec![bare], 0);

        let mut rich = sample_track("1");
        rich.duration_secs = Some(212);
        q.enqueue_or_enrich(rich);
        assert_eq!(q.len(), 1);
        assert_eq!(q.current_track().unwrap().duration_secs, Some(212));

        q.enqueue_or_enrich(sample_track("2"));
        assert_eq!(q.len(), 2);
    }
}
