use std::collections::HashMap;

pub const MAX_CACHED_IMAGES: usize = 36;
pub const IDLE_SOFT_LIMIT: usize = 24;
pub const IDLE_FORGET_FRAMES: u64 = 180;

#[derive(Default)]
pub struct ImageLru {
    frame: u64,
    last_seen: HashMap<String, u64>,
}

impl ImageLru {
    pub fn touch(&mut self, uri: &str) {
        self.last_seen.insert(uri.to_string(), self.frame);
    }

    pub fn len(&self) -> usize {
        self.last_seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.last_seen.is_empty()
    }

    pub fn advance_and_collect_evictions(&mut self) -> Vec<String> {
        self.frame = self.frame.saturating_add(1);
        let mut evicted = Vec::new();
        if self.last_seen.len() > IDLE_SOFT_LIMIT {
            let cutoff = self.frame.saturating_sub(IDLE_FORGET_FRAMES);
            let stale: Vec<String> = self
                .last_seen
                .iter()
                .filter_map(|(k, &seen)| (seen < cutoff).then(|| k.clone()))
                .collect();
            for k in stale {
                self.last_seen.remove(&k);
                evicted.push(k);
            }
        }
        while self.last_seen.len() > MAX_CACHED_IMAGES {
            let Some(oldest) = self
                .last_seen
                .iter()
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
                192
            ),
            "https://lh3.googleusercontent.com/abc=w192-h192-l90-rj"
        );
        assert_eq!(
            optimize_thumb_url(
                "https://yt3.ggpht.com/xyz=s800-c-k-c0x00ffffff-no-rj",
                192
            ),
            "https://yt3.ggpht.com/xyz=s192-c-l90-rj"
        );
        assert_eq!(
            optimize_thumb_url("https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg?sqp=1", 192),
            "https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg?sqp=1"
        );
        assert_eq!(
            optimize_thumb_url("https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg", 544),
            "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"
        );
        assert_eq!(
            optimize_thumb_url("https://example.com/static/cover.png", 192),
            "https://example.com/static/cover.png"
        );
    }

    #[test]
    fn image_lru_caps_entries_and_evicts_stale() {
        let mut lru = ImageLru::default();
        for i in 0..50 {
            lru.touch(&format!("https://img/{i}"));
        }
        let evicted = lru.advance_and_collect_evictions();
        assert_eq!(lru.len(), MAX_CACHED_IMAGES);
        assert_eq!(evicted.len(), 50 - MAX_CACHED_IMAGES);

        for _ in 1..IDLE_FORGET_FRAMES {
            assert!(lru.advance_and_collect_evictions().is_empty());
        }
        let evicted_stale = lru.advance_and_collect_evictions();
        assert_eq!(evicted_stale.len(), MAX_CACHED_IMAGES);
        assert!(lru.is_empty());
    }
}
