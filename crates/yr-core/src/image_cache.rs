use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const MAX_CACHED_IMAGES: usize = 64;
pub const IDLE_SOFT_LIMIT: usize = 24;
pub const IDLE_FORGET_AFTER: Duration = Duration::from_secs(6);
pub const MIN_ACTIVE_AGE: Duration = Duration::from_secs(2);

pub struct ImageLru {
    last_seen: HashMap<String, Instant>,
}

impl Default for ImageLru {
    fn default() -> Self {
        Self {
            last_seen: HashMap::new(),
        }
    }
}

impl ImageLru {
    pub fn touch(&mut self, uri: &str) {
        self.touch_at(uri, Instant::now());
    }

    pub fn touch_at(&mut self, uri: &str, now: Instant) {
        self.last_seen.insert(uri.to_string(), now);
    }

    pub fn len(&self) -> usize {
        self.last_seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.last_seen.is_empty()
    }

    pub fn advance_and_collect_evictions(&mut self) -> Vec<String> {
        self.advance_and_collect_evictions_at(Instant::now())
    }

    pub fn advance_and_collect_evictions_at(&mut self, now: Instant) -> Vec<String> {
        let mut evicted = Vec::new();
        if self.last_seen.len() > IDLE_SOFT_LIMIT {
            let stale: Vec<String> = self
                .last_seen
                .iter()
                .filter_map(|(k, &seen)| {
                    (now.saturating_duration_since(seen) >= IDLE_FORGET_AFTER).then(|| k.clone())
                })
                .collect();
            for k in stale {
                self.last_seen.remove(&k);
                evicted.push(k);
            }
        }
        // Never evict actively-rendered entries (< MIN_ACTIVE_AGE old), preventing
        // per-frame forget_image -> ehttp refetch loops when > MAX_CACHED_IMAGES appear at once.
        while self.last_seen.len() > MAX_CACHED_IMAGES {
            let Some(oldest) = self
                .last_seen
                .iter()
                .filter(|(_, &seen)| now.saturating_duration_since(seen) >= MIN_ACTIVE_AGE)
                .min_by(|(ka, sa), (kb, sb)| sa.cmp(sb).then_with(|| ka.cmp(kb)))
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.last_seen.remove(&oldest);
            evicted.push(oldest);
        }
        evicted
    }
}

/// Rewrites high-resolution YouTube/Google thumbnail URLs to match the UI slot's pixel budget,
/// avoiding decoding multi-megapixel bitmaps into Native Heap and GL texture memory.
pub fn optimize_thumb_url(url: &str, max_px: u32) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let (base, query) = match trimmed.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (trimmed, None),
    };
    if base.contains("googleusercontent.com/") || base.contains("ggpht.com/") {
        let last_slash = base.rfind('/').unwrap_or(0);
        if let Some(eq_rel) = base[last_slash..].find('=') {
            let eq_idx = last_slash + eq_rel;
            let suffix = &base[eq_idx + 1..];
            let starts_with_dim = suffix
                .strip_prefix('w')
                .or_else(|| suffix.strip_prefix('s'))
                .and_then(|rest| rest.chars().next())
                .is_some_and(|c| c.is_ascii_digit());
            if starts_with_dim {
                let is_circle_s = suffix.starts_with('s') && suffix.contains("-c");
                let mut out = String::with_capacity(base.len() + 24);
                out.push_str(&base[..eq_idx]);
                if is_circle_s {
                    out.push_str(&format!("=s{max_px}-c-l90-rj"));
                } else {
                    out.push_str(&format!("=w{max_px}-h{max_px}-l90-rj"));
                }
                if let Some(q) = query {
                    out.push('?');
                    out.push_str(q);
                }
                return out;
            }
        }
    } else if base.contains("i.ytimg.com/vi/") {
        let replacement = if max_px <= 256 {
            Some("/mqdefault.jpg")
        } else if base.ends_with("/maxresdefault.jpg") {
            Some("/hqdefault.jpg")
        } else {
            None
        };
        if let Some(target_file) = replacement {
            for candidate in ["/maxresdefault.jpg", "/sddefault.jpg", "/hqdefault.jpg"] {
                if let Some(prefix) = base.strip_suffix(candidate) {
                    let mut out = format!("{prefix}{target_file}");
                    if let Some(q) = query {
                        out.push('?');
                        out.push_str(q);
                    }
                    return out;
                }
            }
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimize_thumb_url_downscales_googleusercontent_and_ytimg() {
        assert_eq!(
            optimize_thumb_url(
                "https://lh3.googleusercontent.com/abc=w1200-h1200-l90-rj",
                160
            ),
            "https://lh3.googleusercontent.com/abc=w160-h160-l90-rj"
        );
        assert_eq!(
            optimize_thumb_url(
                "https://yt3.ggpht.com/xyz=s800-c-k-c0x00ffffff-no-rj",
                160
            ),
            "https://yt3.ggpht.com/xyz=s160-c-l90-rj"
        );
        assert_eq!(
            optimize_thumb_url("https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg?sqp=1", 160),
            "https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg?sqp=1"
        );
        assert_eq!(
            optimize_thumb_url("https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg", 544),
            "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"
        );
        assert_eq!(
            optimize_thumb_url("https://example.com/static/cover.png", 160),
            "https://example.com/static/cover.png"
        );
    }

    #[test]
    fn image_lru_preserves_active_entries_and_evicts_stale() {
        let t0 = Instant::now();
        let mut lru = ImageLru::default();
        // Touching > MAX_CACHED_IMAGES at t0 never evicts active images immediately.
        for i in 0..80 {
            lru.touch_at(&format!("https://img/{i}"), t0);
        }
        assert!(lru.advance_and_collect_evictions_at(t0).is_empty());
        assert_eq!(lru.len(), 80);

        // After MIN_ACTIVE_AGE while 40..80 are re-touched, inactive 0..40 get trimmed down to MAX_CACHED_IMAGES.
        let t1 = t0 + MIN_ACTIVE_AGE + Duration::from_millis(100);
        for i in 40..80 {
            lru.touch_at(&format!("https://img/{i}"), t1);
        }
        let evicted = lru.advance_and_collect_evictions_at(t1);
        assert_eq!(lru.len(), MAX_CACHED_IMAGES);
        assert_eq!(evicted.len(), 80 - MAX_CACHED_IMAGES);

        // Once all entries sit idle past IDLE_FORGET_AFTER, all evict cleanly.
        let t2 = t1 + IDLE_FORGET_AFTER;
        let evicted_stale = lru.advance_and_collect_evictions_at(t2);
        assert_eq!(evicted_stale.len(), MAX_CACHED_IMAGES);
        assert!(lru.is_empty());
    }
}
