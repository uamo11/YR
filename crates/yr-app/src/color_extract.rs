use eframe::egui::Color32;

/// Compute a rich, legible dark-themed ambient accent color from raw image bytes.
/// Picks a vibrant yet dark-adapted color suitable for Fullscreen Now Playing gradients.
pub fn dominant_ambient_color(bytes: &[u8]) -> Option<Color32> {
    let img = image::load_from_memory(bytes).ok()?;
    let small = img.resize_exact(16, 16, image::imageops::FilterType::Triangle);
    let rgb = small.to_rgb8();

    let mut best_score = -1.0_f32;
    let mut best = [36_u8, 28_u8, 22_u8];
    let mut sum_r = 0_u64;
    let mut sum_g = 0_u64;
    let mut sum_b = 0_u64;
    let mut count = 0_u64;

    for px in rgb.pixels() {
        let [r, g, b] = px.0;
        sum_r += r as u64;
        sum_g += g as u64;
        sum_b += b as u64;
        count += 1;

        let max_c = r.max(g).max(b) as f32;
        let min_c = r.min(g).min(b) as f32;
        let lum = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0;
        let sat = if max_c > 0.0 {
            (max_c - min_c) / max_c
        } else {
            0.0
        };
        if lum < 0.08 || lum > 0.88 {
            continue;
        }
        let score = sat * (0.4 + lum);
        if score > best_score {
            best_score = score;
            best = [r, g, b];
        }
    }

    if best_score < 0.0 && count > 0 {
        best = [
            (sum_r / count) as u8,
            (sum_g / count) as u8,
            (sum_b / count) as u8,
        ];
    }

    // Clamp brightness into a deep moody gradient top color (matching YT Music look).
    let scale = {
        let peak = best[0].max(best[1]).max(best[2]).max(1) as f32;
        (92.0 / peak).min(0.65).max(0.28)
    };
    Some(Color32::from_rgb(
        ((best[0] as f32 * scale) as u8).max(18),
        ((best[1] as f32 * scale) as u8).max(16),
        ((best[2] as f32 * scale) as u8).max(18),
    ))
}
