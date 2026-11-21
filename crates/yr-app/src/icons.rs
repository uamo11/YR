use eframe::egui::{self, Color32, Pos2, Rect, Response, Sense, Ui, Vec2};

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

impl Icon {
    /// Authentic Google Material Design Unicode codepoints
    pub fn as_char(&self) -> char {
        match self {
            Icon::ChevronDown => '\u{e5cf}',      // expand_more
            Icon::ChevronUp => '\u{e5ce}',        // expand_less
            Icon::ChevronRight => '\u{e5cc}',     // chevron_right
            Icon::Close => '\u{e5cd}',            // close
            Icon::Play => '\u{e037}',             // play_arrow
            Icon::Pause => '\u{e034}',            // pause
            Icon::SkipNext => '\u{e044}',         // skip_next
            Icon::SkipPrev => '\u{e045}',         // skip_previous
            Icon::Shuffle => '\u{e043}',          // shuffle
            Icon::Repeat => '\u{e040}',           // repeat
            Icon::RepeatOne => '\u{e041}',        // repeat_one
            Icon::Heart => '\u{e87e}',            // favorite_border
            Icon::HeartFilled => '\u{e87d}',      // favorite
            Icon::Download => '\u{e2c4}',         // file_download
            Icon::Check => '\u{e5ca}',            // check
            Icon::Radio => '\u{e03e}',            // radio
            Icon::PlaylistAdd => '\u{e03b}',      // playlist_add
            Icon::MoreVert => '\u{e5d4}',         // more_vert
            Icon::Home => '\u{e88a}',             // home
            Icon::Search => '\u{e8b6}',           // search
            Icon::Library => '\u{e030}',          // library_music
            Icon::DownloadsNav => '\u{e2c4}',     // file_download
            Icon::Settings => '\u{e8b8}',         // settings
            Icon::Account => '\u{e853}',          // account_circle
            Icon::Moon => '\u{ef44}',             // bedtime
            Icon::Refresh => '\u{e5d5}',          // refresh
            Icon::Plus => '\u{e145}',             // add
            Icon::Share => '\u{e80d}',            // share
            Icon::Album => '\u{e019}',            // album
            Icon::Artist => '\u{e7fd}',           // person
            Icon::Pin => '\u{f10d}',              // push_pin
            Icon::ThumbUp => '\u{e8dc}',          // thumb_up
            Icon::ThumbDown => '\u{e8db}',        // thumb_down
            Icon::Lyrics => '\u{e0b7}',           // chat
            Icon::QueueNext => '\u{e05f}',        // playlist_play
            Icon::QueueAdd => '\u{e03b}',         // playlist_add
            Icon::History => '\u{e889}',          // history
            Icon::ArrowUpLeft => '\u{f1e2}',      // north_west
            Icon::Trending => '\u{e8e5}',         // trending_up
            Icon::NewReleases => '\u{e031}',      // new_releases
            Icon::Mood => '\u{e813}',             // sentiment_satisfied
            Icon::Trash => '\u{e872}',            // delete
            Icon::ArrowBack => '\u{e5c4}',        // arrow_back
            Icon::MusicNote => '\u{e405}',        // music_note
            Icon::Samples => '\u{f06a}',          // smart_display
            Icon::Equalizer => '\u{e01d}',        // equalizer
            Icon::Headphones => '\u{f01f}',       // headphones
            Icon::Cast => '\u{e307}',             // cast
            Icon::Channel => '\u{e851}',          // account_box
            Icon::Recap => '\u{e020}',            // fast_rewind
            Icon::Badge => '\u{e65f}',            // auto_awesome
            Icon::SwitchAccount => '\u{e9ed}',     // switch_account
            Icon::Help => '\u{e8fd}',             // help_outline
            Icon::Video => '\u{f06a}',            // smart_display
            Icon::DataSaver => '\u{eff3}',        // data_saver_on
            Icon::Storage => '\u{e1c2}',          // sd_storage
            Icon::Bell => '\u{e7f4}',             // notifications
            Icon::Shield => '\u{e32a}',           // security
        }
    }
}

/// Renders a crisp Google Material Design vector icon centered inside `rect`.
pub fn draw_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let font_sz = (rect.width().min(rect.height()) * 1.05).max(12.0);
    let font_id = egui::FontId::new(
        font_sz,
        egui::FontFamily::Name(std::sync::Arc::from("material_icons")),
    );
    let mut buf = [0u8; 4];
    let ch_str = icon.as_char().encode_utf8(&mut buf);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        ch_str,
        font_id,
        color,
    );
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

/// Renders an authentic 4-bar animated equalizer visualizer (like YouTube Music's playing indicator).
pub fn draw_animated_equalizer(
    painter: &egui::Painter,
    rect: Rect,
    is_playing: bool,
    time: f64,
    color: Color32,
) {
    let bar_w = 2.4_f32;
    let gap = 2.0_f32;
    let total_w = 4.0 * bar_w + 3.0 * gap;
    let start_x = rect.center().x - total_w * 0.5;
    let max_h = (rect.height() - 4.0).clamp(6.0, 18.0);
    let min_h = 3.5_f32;
    let base_bottom = rect.bottom() - 2.0;

    for i in 0..4 {
        let h = if is_playing {
            let (freq, phase): (f64, f64) = match i {
                0 => (8.5, 0.0),
                1 => (12.0, 1.4),
                2 => (7.2, 2.7),
                _ => (13.5, 0.8),
            };
            let sine = (time * freq + phase).sin().abs() as f32;
            min_h + (max_h - min_h) * sine
        } else {
            match i {
                0 => min_h + 2.0,
                1 => min_h + 5.0,
                2 => min_h + 4.0,
                _ => min_h + 1.5,
            }
        };

        let bx = start_x + (i as f32) * (bar_w + gap);
        let bar_rect = Rect::from_min_max(
            Pos2::new(bx, base_bottom - h),
            Pos2::new(bx + bar_w, base_bottom),
        );
        painter.rect_filled(bar_rect, egui::CornerRadius::same(1), color);
    }
}
