use eframe::egui::{self, Color32, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    ChevronDown,
    ChevronUp,
    Close,
    Play,
    Pause,
    SkipNext,
    SkipPrev,
    Shuffle,
    Repeat,
    RepeatOne,
    Heart,
    HeartFilled,
    Download,
    Check,
    Radio,
    PlaylistAdd,
    MoreVert,
    Home,
    Search,
    Library,
    DownloadsNav,
    Settings,
    Account,
    Moon,
    Refresh,
    Plus,
    Share,
    Album,
    Artist,
    Pin,
    ThumbUp,
    ThumbDown,
    Lyrics,
    QueueNext,
    QueueAdd,
    History,
    ArrowUpLeft,
    Trending,
    NewReleases,
    Mood,
    Trash,
    ArrowBack,
    MusicNote,
    Samples,
    Equalizer,
    Headphones,
    Cast,
    ChevronRight,
    Channel,
    Recap,
    Badge,
    SwitchAccount,
    Help,
    Video,
    DataSaver,
    Storage,
    Bell,
    Shield,
}

/// Renders a crisp vector icon centered inside `rect`.
pub fn draw_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) * 0.5;
    let stroke_w = (s * 0.19).max(1.4);
    let stroke = Stroke::new(stroke_w, color);

    let pt = |dx: f32, dy: f32| Pos2::new(c.x + dx * s, c.y + dy * s);

    match icon {
        Icon::ChevronDown => {
            painter.line_segment([pt(-0.48, -0.18), pt(0.0, 0.30)], stroke);
            painter.line_segment([pt(0.0, 0.30), pt(0.48, -0.18)], stroke);
        }
        Icon::ChevronUp => {
            painter.line_segment([pt(-0.48, 0.18), pt(0.0, -0.30)], stroke);
            painter.line_segment([pt(0.0, -0.30), pt(0.48, 0.18)], stroke);
        }
        Icon::Close => {
            painter.line_segment([pt(-0.38, -0.38), pt(0.38, 0.38)], stroke);
            painter.line_segment([pt(0.38, -0.38), pt(-0.38, 0.38)], stroke);
        }
        Icon::Plus => {
            painter.line_segment([pt(-0.45, 0.0), pt(0.45, 0.0)], stroke);
            painter.line_segment([pt(0.0, -0.45), pt(0.0, 0.45)], stroke);
        }
        Icon::Play => {
            let pts = vec![pt(-0.32, -0.46), pt(0.48, 0.0), pt(-0.32, 0.46)];
            painter.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Pause => {
            let bar_w = (s * 0.28).max(2.5);
            let left = Rect::from_center_size(pt(-0.26, 0.0), Vec2::new(bar_w, s * 0.96));
            let right = Rect::from_center_size(pt(0.26, 0.0), Vec2::new(bar_w, s * 0.96));
            painter.rect_filled(left, 2.0, color);
            painter.rect_filled(right, 2.0, color);
        }
        Icon::SkipNext => {
            let pts = vec![pt(-0.45, -0.40), pt(0.18, 0.0), pt(-0.45, 0.40)];
            painter.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
            let bar = Rect::from_center_size(pt(0.34, 0.0), Vec2::new((s * 0.20).max(2.2), s * 0.80));
            painter.rect_filled(bar, 1.5, color);
        }
        Icon::SkipPrev => {
            let pts = vec![pt(0.45, -0.40), pt(-0.18, 0.0), pt(0.45, 0.40)];
            painter.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
            let bar = Rect::from_center_size(pt(-0.34, 0.0), Vec2::new((s * 0.20).max(2.2), s * 0.80));
            painter.rect_filled(bar, 1.5, color);
        }
        Icon::Shuffle => {
            painter.line_segment([pt(-0.52, -0.30), pt(-0.15, -0.30)], stroke);
            painter.line_segment([pt(-0.15, -0.30), pt(0.18, 0.30)], stroke);
            painter.line_segment([pt(0.18, 0.30), pt(0.50, 0.30)], stroke);
            painter.line_segment([pt(-0.52, 0.30), pt(-0.15, 0.30)], stroke);
            painter.line_segment([pt(-0.15, 0.30), pt(0.18, -0.30)], stroke);
            painter.line_segment([pt(0.18, -0.30), pt(0.50, -0.30)], stroke);
            // Arrowheads
            painter.line_segment([pt(0.36, -0.44), pt(0.52, -0.30)], stroke);
            painter.line_segment([pt(0.36, -0.16), pt(0.52, -0.30)], stroke);
            painter.line_segment([pt(0.36, 0.16), pt(0.52, 0.30)], stroke);
            painter.line_segment([pt(0.36, 0.44), pt(0.52, 0.30)], stroke);
        }
        Icon::Repeat | Icon::RepeatOne => {
            // Top & bottom horizontal bars with rounded corners
            painter.line_segment([pt(-0.38, -0.30), pt(0.36, -0.30)], stroke);
            painter.line_segment([pt(-0.46, -0.05), pt(-0.46, -0.22)], stroke);
            painter.line_segment([pt(-0.46, -0.22), pt(-0.38, -0.30)], stroke);
            painter.line_segment([pt(0.24, -0.44), pt(0.40, -0.30)], stroke);
            painter.line_segment([pt(0.24, -0.16), pt(0.40, -0.30)], stroke);

            painter.line_segment([pt(-0.36, 0.30), pt(0.38, 0.30)], stroke);
            painter.line_segment([pt(0.46, 0.05), pt(0.46, 0.22)], stroke);
            painter.line_segment([pt(0.46, 0.22), pt(0.38, 0.30)], stroke);
            painter.line_segment([pt(-0.24, 0.16), pt(-0.40, 0.30)], stroke);
            painter.line_segment([pt(-0.24, 0.44), pt(-0.40, 0.30)], stroke);

            if matches!(icon, Icon::RepeatOne) {
                painter.line_segment([pt(0.0, -0.16), pt(0.0, 0.16)], stroke);
            }
        }
        Icon::Heart | Icon::HeartFilled => {
            let pts = vec![
                pt(0.0, 0.48),
                pt(-0.46, 0.05),
                pt(-0.48, -0.22),
                pt(-0.26, -0.42),
                pt(0.0, -0.22),
                pt(0.26, -0.42),
                pt(0.48, -0.22),
                pt(0.46, 0.05),
            ];
            if matches!(icon, Icon::HeartFilled) {
                painter.add(egui::Shape::closed_line(pts.clone(), stroke));
                painter.circle_filled(pt(-0.22, -0.15), s * 0.24, color);
                painter.circle_filled(pt(0.22, -0.15), s * 0.24, color);
                let tri = vec![pt(-0.42, -0.02), pt(0.42, -0.02), pt(0.0, 0.46)];
                painter.add(egui::Shape::convex_polygon(tri, color, Stroke::NONE));
            } else {
                painter.add(egui::Shape::closed_line(pts, stroke));
            }
        }
        Icon::Download => {
            painter.line_segment([pt(0.0, -0.46), pt(0.0, 0.20)], stroke);
            painter.line_segment([pt(-0.28, -0.06), pt(0.0, 0.22)], stroke);
            painter.line_segment([pt(0.28, -0.06), pt(0.0, 0.22)], stroke);
            painter.line_segment([pt(-0.44, 0.44), pt(0.44, 0.44)], stroke);
            painter.line_segment([pt(-0.44, 0.26), pt(-0.44, 0.44)], stroke);
            painter.line_segment([pt(0.44, 0.26), pt(0.44, 0.44)], stroke);
        }
        Icon::Check => {
            painter.line_segment([pt(-0.42, 0.02), pt(-0.12, 0.34)], stroke);
            painter.line_segment([pt(-0.12, 0.34), pt(0.46, -0.30)], stroke);
        }
        Icon::Radio => {
            painter.circle_filled(c, s * 0.14, color);
            for r in [0.32_f32, 0.54_f32] {
                let arc_l: Vec<Pos2> = (-4..=4)
                    .map(|i| {
                        let a = std::f32::consts::PI + (i as f32 / 4.0) * 0.85;
                        pt(a.cos() * r, a.sin() * r)
                    })
                    .collect();
                painter.add(egui::Shape::line(arc_l, stroke));
                let arc_r: Vec<Pos2> = (-4..=4)
                    .map(|i| {
                        let a = (i as f32 / 4.0) * 0.85;
                        pt(a.cos() * r, a.sin() * r)
                    })
                    .collect();
                painter.add(egui::Shape::line(arc_r, stroke));
            }
        }
        Icon::PlaylistAdd => {
            painter.line_segment([pt(-0.50, -0.30), pt(0.20, -0.30)], stroke);
            painter.line_segment([pt(-0.50, 0.0), pt(0.20, 0.0)], stroke);
            painter.line_segment([pt(-0.50, 0.30), pt(0.0, 0.30)], stroke);
            painter.line_segment([pt(0.20, 0.26), pt(0.56, 0.26)], stroke);
            painter.line_segment([pt(0.38, 0.08), pt(0.38, 0.44)], stroke);
        }
        Icon::MoreVert => {
            let r = (s * 0.14).max(1.8);
            painter.circle_filled(pt(0.0, -0.40), r, color);
            painter.circle_filled(pt(0.0, 0.0), r, color);
            painter.circle_filled(pt(0.0, 0.40), r, color);
        }
        Icon::Home => {
            let roof = vec![pt(-0.52, -0.02), pt(0.0, -0.48), pt(0.52, -0.02)];
            painter.add(egui::Shape::line(roof, stroke));
            let body = vec![
                pt(-0.36, -0.08),
                pt(-0.36, 0.44),
                pt(0.36, 0.44),
                pt(0.36, -0.08),
            ];
            painter.add(egui::Shape::line(body, stroke));
        }
        Icon::Search => {
            painter.circle_stroke(pt(-0.10, -0.10), s * 0.34, stroke);
            painter.line_segment([pt(0.14, 0.14), pt(0.46, 0.46)], stroke);
        }
        Icon::Library => {
            let pts = vec![
                pt(-0.34, -0.46),
                pt(0.34, -0.46),
                pt(0.34, 0.46),
                pt(0.0, 0.22),
                pt(-0.34, 0.46),
            ];
            painter.add(egui::Shape::closed_line(pts, stroke));
        }
        Icon::DownloadsNav => {
            painter.circle_stroke(c, s * 0.48, stroke);
            painter.line_segment([pt(0.0, -0.26), pt(0.0, 0.18)], stroke);
            painter.line_segment([pt(-0.20, 0.0), pt(0.0, 0.20)], stroke);
            painter.line_segment([pt(0.20, 0.0), pt(0.0, 0.20)], stroke);
        }
        Icon::Settings => {
            painter.circle_stroke(c, s * 0.26, stroke);
            for i in 0..8 {
                let ang = (i as f32) * std::f32::consts::FRAC_PI_4;
                let (sin, cos) = ang.sin_cos();
                painter.line_segment([pt(cos * 0.34, sin * 0.34), pt(cos * 0.50, sin * 0.50)], stroke);
            }
        }
        Icon::Account => {
            painter.circle_stroke(c, s * 0.48, stroke);
            painter.circle_stroke(pt(0.0, -0.12), s * 0.18, stroke);
            let arc: Vec<Pos2> = (-3..=3)
                .map(|i| {
                    let x = (i as f32) * 0.10;
                    let y = 0.36 - (1.0 - (x / 0.32).powi(2)).max(0.0).sqrt() * 0.16;
                    pt(x, y)
                })
                .collect();
            painter.add(egui::Shape::line(arc, stroke));
        }
        Icon::Moon => {
            let mut pts = Vec::new();
            for i in 0..=16 {
                let a = -1.8 + (i as f32 / 16.0) * 3.6;
                pts.push(pt(0.08 + a.cos() * 0.42, a.sin() * 0.42));
            }
            for i in (0..=16).rev() {
                let a = -1.5 + (i as f32 / 16.0) * 3.0;
                pts.push(pt(0.24 + a.cos() * 0.32, a.sin() * 0.34));
            }
            painter.add(egui::Shape::closed_line(pts, stroke));
        }
        Icon::Refresh => {
            let mut pts = Vec::new();
            for i in 0..=12 {
                let a = -0.3 + (i as f32) * 0.42;
                pts.push(pt(a.cos() * 0.40, a.sin() * 0.40));
            }
            painter.add(egui::Shape::line(pts, stroke));
            painter.line_segment([pt(0.22, -0.34), pt(0.42, -0.14)], stroke);
            painter.line_segment([pt(0.42, -0.38), pt(0.42, -0.14)], stroke);
        }
        Icon::Share => {
            // Curved right-pointing share arrow
            painter.line_segment([pt(-0.42, 0.34), pt(-0.34, 0.04)], stroke);
            painter.line_segment([pt(-0.34, 0.04), pt(0.06, -0.10)], stroke);
            painter.line_segment([pt(0.06, -0.10), pt(0.42, -0.10)], stroke);
            painter.line_segment([pt(0.16, -0.34), pt(0.44, -0.10)], stroke);
            painter.line_segment([pt(0.16, 0.14), pt(0.44, -0.10)], stroke);
        }
        Icon::Album => {
            painter.circle_stroke(c, s * 0.46, stroke);
            painter.circle_stroke(c, s * 0.15, stroke);
        }
        Icon::Artist => {
            painter.circle_stroke(pt(-0.12, -0.16), s * 0.20, stroke);
            let shoulder: Vec<Pos2> = (-3..=3)
                .map(|i| {
                    let x = -0.12 + (i as f32) * 0.09;
                    let y = 0.40 - (1.0 - ((x + 0.12) / 0.28).powi(2)).max(0.0).sqrt() * 0.18;
                    pt(x, y)
                })
                .collect();
            painter.add(egui::Shape::line(shoulder, stroke));
            painter.circle_filled(pt(0.32, 0.26), s * 0.10, color);
            painter.line_segment([pt(0.40, 0.26), pt(0.40, -0.12)], stroke);
            painter.line_segment([pt(0.40, -0.12), pt(0.52, -0.04)], stroke);
        }
        Icon::Pin => {
            let head = vec![pt(-0.26, -0.42), pt(0.26, -0.42)];
            painter.line_segment([head[0], head[1]], stroke);
            let body = vec![
                pt(-0.16, -0.42),
                pt(-0.22, -0.06),
                pt(0.22, -0.06),
                pt(0.16, -0.42),
            ];
            painter.add(egui::Shape::closed_line(body, stroke));
            painter.line_segment([pt(-0.30, -0.06), pt(0.30, -0.06)], stroke);
            painter.line_segment([pt(0.0, -0.06), pt(0.0, 0.44)], stroke);
        }
        Icon::ThumbUp => {
            let cuff = Rect::from_min_max(pt(-0.46, -0.02), pt(-0.28, 0.44));
            painter.rect_stroke(cuff, 1.0, stroke, egui::StrokeKind::Middle);
            let pts = vec![
                pt(-0.24, 0.42),
                pt(0.26, 0.42),
                pt(0.42, 0.08),
                pt(0.38, -0.08),
                pt(0.08, -0.08),
                pt(0.14, -0.38),
                pt(0.02, -0.44),
                pt(-0.16, -0.16),
                pt(-0.24, -0.02),
            ];
            painter.add(egui::Shape::closed_line(pts, stroke));
        }
        Icon::ThumbDown => {
            let cuff = Rect::from_min_max(pt(-0.46, -0.44), pt(-0.28, 0.02));
            painter.rect_stroke(cuff, 1.0, stroke, egui::StrokeKind::Middle);
            let pts = vec![
                pt(-0.24, -0.42),
                pt(0.26, -0.42),
                pt(0.42, -0.08),
                pt(0.38, 0.08),
                pt(0.08, 0.08),
                pt(0.14, 0.38),
                pt(0.02, 0.44),
                pt(-0.16, 0.16),
                pt(-0.24, 0.02),
            ];
            painter.add(egui::Shape::closed_line(pts, stroke));
        }
        Icon::Lyrics => {
            for dx in [-0.22_f32, 0.16_f32] {
                let center_quote = pt(dx, -0.06);
                painter.circle_filled(center_quote, s * 0.16, color);
                let tail = vec![
                    pt(dx - 0.10, -0.04),
                    pt(dx - 0.14, 0.24),
                    pt(dx - 0.02, 0.08),
                ];
                painter.add(egui::Shape::convex_polygon(tail, color, Stroke::NONE));
            }
        }
        Icon::QueueNext => {
            painter.line_segment([pt(-0.44, -0.26), pt(0.16, -0.26)], stroke);
            painter.line_segment([pt(-0.44, 0.02), pt(0.44, 0.02)], stroke);
            painter.line_segment([pt(-0.44, 0.30), pt(0.44, 0.30)], stroke);
            let tri = vec![pt(0.24, -0.44), pt(0.48, -0.26), pt(0.24, -0.08)];
            painter.add(egui::Shape::convex_polygon(tri, color, Stroke::NONE));
        }
        Icon::QueueAdd => {
            painter.line_segment([pt(-0.44, -0.26), pt(0.44, -0.26)], stroke);
            painter.line_segment([pt(-0.44, 0.02), pt(0.44, 0.02)], stroke);
            painter.line_segment([pt(-0.44, 0.30), pt(0.04, 0.30)], stroke);
            let tri = vec![pt(0.18, 0.14), pt(0.46, 0.30), pt(0.18, 0.46)];
            painter.add(egui::Shape::convex_polygon(tri, color, Stroke::NONE));
        }
        Icon::History => {
            let mut pts = Vec::new();
            for i in 0..=11 {
                let a = -2.4 + (i as f32) * 0.46;
                pts.push(pt(a.cos() * 0.42, a.sin() * 0.42));
            }
            painter.add(egui::Shape::line(pts, stroke));
            painter.line_segment([pt(-0.42, -0.10), pt(-0.42, -0.34)], stroke);
            painter.line_segment([pt(-0.42, -0.10), pt(-0.18, -0.14)], stroke);
            painter.line_segment([pt(0.0, -0.22), pt(0.0, 0.04)], stroke);
            painter.line_segment([pt(0.0, 0.04), pt(0.18, 0.16)], stroke);
        }
        Icon::ArrowUpLeft => {
            painter.line_segment([pt(-0.34, -0.34), pt(0.36, 0.36)], stroke);
            painter.line_segment([pt(-0.34, -0.34), pt(0.06, -0.34)], stroke);
            painter.line_segment([pt(-0.34, -0.34), pt(-0.34, 0.06)], stroke);
        }
        Icon::Trending => {
            let pts = vec![
                pt(-0.46, 0.28),
                pt(-0.10, -0.06),
                pt(0.10, 0.12),
                pt(0.44, -0.26),
            ];
            painter.add(egui::Shape::line(pts, stroke));
            painter.line_segment([pt(0.20, -0.26), pt(0.44, -0.26)], stroke);
            painter.line_segment([pt(0.44, -0.26), pt(0.44, -0.02)], stroke);
        }
        Icon::NewReleases => {
            painter.circle_stroke(c, s * 0.36, stroke);
            for i in 0..8 {
                let a = (i as f32) * std::f32::consts::FRAC_PI_4;
                let (sn, cs) = a.sin_cos();
                painter.line_segment([pt(cs * 0.36, sn * 0.36), pt(cs * 0.50, sn * 0.50)], stroke);
            }
        }
        Icon::Mood => {
            painter.circle_stroke(c, s * 0.44, stroke);
            painter.circle_filled(pt(-0.15, -0.10), s * 0.07, color);
            painter.circle_filled(pt(0.15, -0.10), s * 0.07, color);
            let smile = vec![pt(-0.18, 0.12), pt(0.0, 0.24), pt(0.18, 0.12)];
            painter.add(egui::Shape::line(smile, stroke));
        }
        Icon::Trash => {
            painter.line_segment([pt(-0.36, -0.26), pt(0.36, -0.26)], stroke);
            painter.line_segment([pt(-0.14, -0.40), pt(0.14, -0.40)], stroke);
            painter.line_segment([pt(-0.26, -0.26), pt(-0.22, 0.40)], stroke);
            painter.line_segment([pt(0.26, -0.26), pt(0.22, 0.40)], stroke);
            painter.line_segment([pt(-0.22, 0.40), pt(0.22, 0.40)], stroke);
        }
        Icon::ArrowBack => {
            painter.line_segment([pt(-0.42, 0.0), pt(0.44, 0.0)], stroke);
            painter.line_segment([pt(-0.42, 0.0), pt(-0.08, -0.34)], stroke);
            painter.line_segment([pt(-0.42, 0.0), pt(-0.08, 0.34)], stroke);
        }
        Icon::MusicNote => {
            painter.circle_filled(pt(-0.24, 0.34), s * 0.17, color);
            painter.circle_filled(pt(0.24, 0.24), s * 0.17, color);
            painter.line_segment([pt(-0.09, 0.34), pt(-0.09, -0.32)], stroke);
            painter.line_segment([pt(0.39, 0.24), pt(0.39, -0.42)], stroke);
            painter.line_segment([pt(-0.09, -0.32), pt(0.39, -0.42)], stroke);
        }
        Icon::Samples => {
            // Double forward-play outline matching YouTube Music's Samples tab icon.
            let front = [pt(-0.06, -0.38), pt(0.46, 0.0), pt(-0.06, 0.38)];
            painter.add(egui::Shape::closed_line(front.to_vec(), stroke));
            let back = [pt(-0.46, -0.34), pt(0.02, 0.0), pt(-0.46, 0.34)];
            painter.line_segment([back[0], back[1]], stroke);
            painter.line_segment([back[1], back[2]], stroke);
        }
        Icon::Equalizer => {
            // 3 vertical slider tracks with rectangular fader knobs
            for &(x_rel, knob_y) in &[(-0.32, -0.12), (0.0, 0.18), (0.32, -0.22)] {
                painter.line_segment([pt(x_rel, -0.42), pt(x_rel, 0.42)], stroke);
                let knob_rect = Rect::from_center_size(pt(x_rel, knob_y), Vec2::new(s * 0.32, s * 0.18));
                painter.rect_filled(knob_rect, 2.0, color);
            }
        }
        Icon::Headphones => {
            // Headband arc
            let arc_pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let angle = std::f32::consts::PI + (i as f32 / 12.0) * std::f32::consts::PI;
                    pt(angle.cos() * 0.36, angle.sin() * 0.32 - 0.05)
                })
                .collect();
            for pair in arc_pts.windows(2) {
                painter.line_segment([pair[0], pair[1]], stroke);
            }
            // Earcups
            let cup_w = s * 0.18;
            let cup_h = s * 0.42;
            painter.rect_filled(
                Rect::from_center_size(pt(-0.36, 0.12), Vec2::new(cup_w, cup_h)),
                cup_w * 0.4,
                color,
            );
            painter.rect_filled(
                Rect::from_center_size(pt(0.36, 0.12), Vec2::new(cup_w, cup_h)),
                cup_w * 0.4,
                color,
            );
        }
        Icon::Cast => {
            // Cast screen frame with open bottom-left
            painter.line_segment([pt(-0.42, 0.05), pt(-0.42, -0.34)], stroke);
            painter.line_segment([pt(-0.42, -0.34), pt(0.42, -0.34)], stroke);
            painter.line_segment([pt(0.42, -0.34), pt(0.42, 0.34)], stroke);
            painter.line_segment([pt(0.42, 0.34), pt(-0.05, 0.34)], stroke);
            // Broadcast dot and arcs in bottom left
            painter.circle_filled(pt(-0.38, 0.30), s * 0.07, color);
            let arc1: Vec<Pos2> = (0..=6)
                .map(|i| {
                    let a = -(i as f32 / 6.0) * (std::f32::consts::PI * 0.5);
                    pt(-0.38 + a.cos() * 0.18, 0.30 + a.sin() * 0.18)
                })
                .collect();
            for pair in arc1.windows(2) {
                painter.line_segment([pair[0], pair[1]], stroke);
            }
            let arc2: Vec<Pos2> = (0..=8)
                .map(|i| {
                    let a = -(i as f32 / 8.0) * (std::f32::consts::PI * 0.5);
                    pt(-0.38 + a.cos() * 0.32, 0.30 + a.sin() * 0.32)
                })
                .collect();
            for pair in arc2.windows(2) {
                painter.line_segment([pair[0], pair[1]], stroke);
            }
        }
        Icon::ChevronRight => {
            painter.line_segment([pt(-0.18, -0.42), pt(0.30, 0.0)], stroke);
            painter.line_segment([pt(0.30, 0.0), pt(-0.18, 0.42)], stroke);
        }
        Icon::Channel => {
            let card = Rect::from_center_size(c, Vec2::new(s * 1.5, s * 1.3));
            painter.rect_stroke(card, 3.0, stroke, egui::StrokeKind::Outside);
            painter.circle_filled(pt(0.0, -0.15), s * 0.22, color);
            painter.line_segment([pt(-0.35, 0.35), pt(0.35, 0.35)], stroke);
        }
        Icon::Recap => {
            let t1 = vec![pt(-0.04, 0.0), pt(0.36, -0.36), pt(0.36, 0.36)];
            let t2 = vec![pt(-0.46, 0.0), pt(-0.06, -0.36), pt(-0.06, 0.36)];
            painter.add(egui::Shape::convex_polygon(t1, color, Stroke::NONE));
            painter.add(egui::Shape::convex_polygon(t2, color, Stroke::NONE));
        }
        Icon::Badge => {
            painter.line_segment([pt(0.0, -0.48), pt(0.0, 0.48)], stroke);
            painter.line_segment([pt(-0.48, 0.0), pt(0.48, 0.0)], stroke);
            painter.line_segment([pt(-0.32, -0.32), pt(0.32, 0.32)], stroke);
            painter.line_segment([pt(-0.32, 0.32), pt(0.32, -0.32)], stroke);
        }
        Icon::SwitchAccount => {
            painter.circle_stroke(pt(-0.10, -0.10), s * 0.26, stroke);
            painter.line_segment([pt(-0.42, 0.38), pt(0.20, 0.38)], stroke);
            painter.line_segment([pt(0.25, -0.38), pt(0.48, -0.38)], stroke);
            painter.line_segment([pt(0.38, -0.48), pt(0.48, -0.38)], stroke);
            painter.line_segment([pt(0.38, -0.28), pt(0.48, -0.38)], stroke);
        }
        Icon::Help => {
            painter.circle_stroke(c, s * 0.48, stroke);
            painter.line_segment([pt(-0.12, -0.20), pt(0.0, -0.28)], stroke);
            painter.line_segment([pt(0.0, -0.28), pt(0.14, -0.18)], stroke);
            painter.line_segment([pt(0.14, -0.18), pt(0.0, 0.05)], stroke);
            painter.circle_filled(pt(0.0, 0.24), stroke_w * 0.7, color);
        }
        Icon::Video => {
            let screen = Rect::from_center_size(c, Vec2::new(s * 1.5, s * 1.05));
            painter.rect_stroke(screen, 3.0, stroke, egui::StrokeKind::Outside);
            let tri = vec![pt(-0.12, -0.20), pt(0.20, 0.0), pt(-0.12, 0.20)];
            painter.add(egui::Shape::convex_polygon(tri, color, Stroke::NONE));
        }
        Icon::DataSaver => {
            let batt = Rect::from_center_size(c, Vec2::new(s * 1.4, s * 0.85));
            painter.rect_stroke(batt, 2.0, stroke, egui::StrokeKind::Outside);
            painter.line_segment([pt(-0.40, 0.0), pt(0.10, 0.0)], stroke);
        }
        Icon::Storage => {
            painter.rect_stroke(
                Rect::from_center_size(pt(0.0, -0.24), Vec2::new(s * 1.4, s * 0.38)),
                2.0,
                stroke,
                egui::StrokeKind::Outside,
            );
            painter.rect_stroke(
                Rect::from_center_size(pt(0.0, 0.24), Vec2::new(s * 1.4, s * 0.38)),
                2.0,
                stroke,
                egui::StrokeKind::Outside,
            );
        }
        Icon::Bell => {
            painter.line_segment([pt(-0.35, 0.24), pt(0.35, 0.24)], stroke);
            painter.line_segment([pt(-0.25, 0.24), pt(-0.20, -0.14)], stroke);
            painter.line_segment([pt(0.25, 0.24), pt(0.20, -0.14)], stroke);
            painter.line_segment([pt(-0.20, -0.14), pt(0.0, -0.38)], stroke);
            painter.line_segment([pt(0.20, -0.14), pt(0.0, -0.38)], stroke);
            painter.circle_filled(pt(0.0, 0.38), stroke_w * 0.8, color);
        }
        Icon::Shield => {
            let sh = vec![
                pt(0.0, -0.45),
                pt(0.38, -0.28),
                pt(0.30, 0.15),
                pt(0.0, 0.45),
                pt(-0.30, 0.15),
                pt(-0.38, -0.28),
            ];
            painter.add(egui::Shape::closed_line(sh, stroke));
        }
    }
}

/// Square clickable icon button with rounded hover/fill surface.
pub fn icon_button(
    ui: &mut Ui,
    icon: Icon,
    box_size: f32,
    icon_size: f32,
    fill: Color32,
    fg: Color32,
    rounding: f32,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(box_size), Sense::click());
    if ui.is_rect_visible(rect) {
        let bg = if resp.hovered() {
            fill.gamma_multiply(1.25)
        } else {
            fill
        };
        if bg != Color32::TRANSPARENT {
            ui.painter().rect_filled(rect, rounding, bg);
        }
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(icon_size));
        draw_icon(ui.painter(), icon_rect, icon, fg);
    }
    resp
}

/// Pill button with a leading vector icon and short text label.
pub fn icon_pill_button(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    fill: Color32,
    fg: Color32,
) -> Response {
    let font_id = egui::FontId::proportional(13.0);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font_id, fg);
    let pad_x = 12.0;
    let icon_sz = 15.0;
    let gap = if label.is_empty() { 0.0 } else { 6.0 };
    let w = pad_x * 2.0 + icon_sz + gap + galley.size().x;
    let h = 32.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    if ui.is_rect_visible(rect) {
        let bg = if resp.hovered() {
            fill.gamma_multiply(1.2)
        } else {
            fill
        };
        ui.painter().rect_filled(rect, 16.0, bg);
        let icon_rect = Rect::from_center_size(
            Pos2::new(rect.left() + pad_x + icon_sz * 0.5, rect.center().y),
            Vec2::splat(icon_sz),
        );
        draw_icon(ui.painter(), icon_rect, icon, fg);
        if !label.is_empty() {
            let text_pos = Pos2::new(
                icon_rect.right() + gap,
                rect.center().y - galley.size().y * 0.5,
            );
            ui.painter().galley(text_pos, galley, fg);
        }
    }
    resp
}
