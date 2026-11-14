use egui::{Color32, CornerRadius, Margin, Stroke, Style, Visuals};

pub const BG_PRIMARY: Color32 = Color32::from_rgb(3, 3, 3);
pub const BG_SURFACE: Color32 = Color32::from_rgb(18, 18, 18);
pub const BG_ELEVATED: Color32 = Color32::from_rgb(33, 33, 33);
pub const BG_HIGHLIGHT: Color32 = Color32::from_rgb(48, 48, 48);
pub const ACCENT_RED: Color32 = Color32::from_rgb(255, 0, 51);
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(245, 245, 245);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(165, 165, 165);
pub const GREEN_OK: Color32 = Color32::from_rgb(46, 204, 113);

pub fn apply_ytm_theme(ctx: &egui::Context) {
    let mut style: Style = (*ctx.style()).clone();
    let mut visuals = Visuals::dark();

    visuals.panel_fill = BG_PRIMARY;
    visuals.window_fill = BG_SURFACE;
    visuals.extreme_bg_color = BG_SURFACE;
    visuals.faint_bg_color = BG_ELEVATED;

    visuals.widgets.noninteractive.bg_fill = BG_SURFACE;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(10);

    visuals.widgets.inactive.bg_fill = BG_ELEVATED;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(10);

    visuals.widgets.hovered.bg_fill = BG_HIGHLIGHT;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.2, Color32::WHITE);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(10);

    visuals.widgets.active.bg_fill = ACCENT_RED;
    visuals.widgets.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    visuals.widgets.active.corner_radius = CornerRadius::same(10);

    visuals.selection.bg_fill = ACCENT_RED;
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);

    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(12.0, 8.0);
    style.spacing.window_margin = Margin::same(12);

    ctx.set_style(style);
}
