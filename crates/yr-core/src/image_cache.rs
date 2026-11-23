use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const MAX_CACHED_IMAGES: usize = 64;
pub const IDLE_SOFT_LIMIT: usize = 32;
pub const IDLE_FORGET_AFTER: Duration = Duration::from_secs(45);
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

/// Resolves a track thumbnail URL, normalizing scheme-relative URLs and falling
/// back to YouTube's static cover endpoint when metadata omits a thumbnail.
pub fn track_thumb_url(thumb: Option<&str>, track_id: &str) -> Option<String> {
    if let Some(raw) = thumb.map(str::trim).filter(|s| !s.is_empty()) {
        if raw.starts_with("//") {
            return Some(format!("https:{raw}"));
        }
        return Some(raw.to_string());
    }
    let id = track_id.trim();
    if id.len() == 11
        && !id.starts_with("local_")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Some(format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"))
    } else {
        None
    }
}

/// Returns the local disk path for a cached offline track cover (`<cover_dir>/<safe_id>.jpg`).
pub fn offline_cover_path(cover_dir: &std::path::Path, track_id: &str) -> std::path::PathBuf {
    let safe: String = track_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect();
    let name = if safe.is_empty() { "track" } else { safe.as_str() };
    cover_dir.join(format!("{name}.jpg"))
}

/// Formats a local filesystem path as a `file://` URI accepted by `egui_extras` file loader.
pub fn file_uri_for_path(path: &std::path::Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

/// Computes `(u0, v0, u1, v1)` center-crop UV coordinates so cover art fills `dst_w × dst_h`
/// without stretching or showing YouTube's baked-in 4:3 letterbox bars (`hqdefault`/`sddefault`).
pub fn compute_cover_crop_uv(tex_w: f32, tex_h: f32, dst_w: f32, dst_h: f32) -> (f32, f32, f32, f32) {
    if tex_w <= 0.0 || tex_h <= 0.0 || dst_w <= 0.0 || dst_h <= 0.0 {
        return (0.0, 0.0, 1.0, 1.0);
    }
    let is_yt_letterbox_4_3 = ((tex_w - 480.0).abs() < 1.5 && (tex_h - 360.0).abs() < 1.5)
        || ((tex_w - 640.0).abs() < 1.5 && (tex_h - 480.0).abs() < 1.5);
    let (base_v0, base_v1, eff_h) = if is_yt_letterbox_4_3 {
        (0.125_f32, 0.875_f32, tex_h * 0.75)
    } else {
        (0.0_f32, 1.0_f32, tex_h)
    };
    let tex_aspect = tex_w / eff_h;
    let dst_aspect = dst_w / dst_h;
    if (tex_aspect - dst_aspect).abs() <= 0.02 {
        return (0.0, base_v0, 1.0, base_v1);
    }
    if tex_aspect > dst_aspect {
        let keep = (dst_aspect / tex_aspect).clamp(0.1, 1.0);
        let margin = (1.0 - keep) * 0.5;
        (margin, base_v0, 1.0 - margin, base_v1)
    } else {
        let keep = (tex_aspect / dst_aspect).clamp(0.1, 1.0);
        let base_h = base_v1 - base_v0;
        let margin = base_h * (1.0 - keep) * 0.5;
        (0.0, base_v0 + margin, 1.0, base_v1 - margin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_thumb_url_falls_back_to_ytimg_when_missing() {
        assert_eq!(
            track_thumb_url(None, "dQw4w9WgXcQ"),
            Some("https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg".to_string())
        );
        assert_eq!(
            track_thumb_url(Some("//lh3.googleusercontent.com/abc"), "dQw4w9WgXcQ"),
            Some("https://lh3.googleusercontent.com/abc".to_string())
        );
        assert_eq!(track_thumb_url(None, "local_demo_1"), None);
    }

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

        // After MIN_ACTIVE_AGE while 50..80 (30 entries) are re-touched, inactive 0..50 get trimmed down to MAX_CACHED_IMAGES.
        let t1 = t0 + MIN_ACTIVE_AGE + Duration::from_millis(100);
        for i in 50..80 {
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

    #[test]
    fn offline_cover_path_sanitizes_and_formats_file_uri() {
        let dir = std::path::Path::new("/tmp/covers");
        let p = offline_cover_path(dir, "../a/b");
        assert_eq!(p, dir.join("ab.jpg"));

        let clean = offline_cover_path(dir, "abc123");
        assert_eq!(clean, dir.join("abc123.jpg"));
        assert_eq!(file_uri_for_path(&clean), "file:///tmp/covers/abc123.jpg");
    }

    #[test]
    fn compute_cover_crop_uv_removes_yt_letterbox_bars_and_crops_16_9() {
        assert_eq!(
            compute_cover_crop_uv(544.0, 544.0, 148.0, 148.0),
            (0.0, 0.0, 1.0, 1.0)
        );

        let (u0, v0, u1, v1) = compute_cover_crop_uv(320.0, 180.0, 100.0, 100.0);
        assert!((u0 - 0.21875).abs() < 1e-4);
        assert!((u1 - 0.78125).abs() < 1e-4);
        assert_eq!(v0, 0.0);
        assert_eq!(v1, 1.0);

        // 480x360 hqdefault into 1:1 strips top/bottom 12.5% black bars AND crops 16:9 sides to square
        let (u0, v0, u1, v1) = compute_cover_crop_uv(480.0, 360.0, 100.0, 100.0);
        assert!((v0 - 0.125).abs() < 1e-5);
        assert!((v1 - 0.875).abs() < 1e-5);
        assert!((u0 - 0.21875).abs() < 1e-4);
        assert!((u1 - 0.78125).abs() < 1e-4);

        // 480x360 hqdefault into 16:9 leaves only the active 16:9 band
        let (u0, v0, u1, v1) = compute_cover_crop_uv(480.0, 360.0, 320.0, 180.0);
        assert_eq!(u0, 0.0);
        assert_eq!(u1, 1.0);
        assert!((v0 - 0.125).abs() < 1e-5);
        assert!((v1 - 0.875).abs() < 1e-5);
    }
}
