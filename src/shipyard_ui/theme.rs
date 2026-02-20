use eframe::egui::{self, Color32};

pub fn configure_theme(context: &egui::Context) {
    let mut style = (*context.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.visuals.window_fill = Color32::from_rgb(244, 245, 247);
    style.visuals.panel_fill = Color32::from_rgb(244, 245, 247);
    style.visuals.extreme_bg_color = Color32::from_rgb(239, 241, 244);
    style.visuals.override_text_color = Some(Color32::from_rgb(42, 47, 54));
    style.visuals.widgets.noninteractive.fg_stroke.color = Color32::from_rgb(52, 57, 64);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(232, 234, 238);
    style.visuals.widgets.inactive.fg_stroke.color = Color32::from_rgb(50, 55, 62);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(224, 235, 252);
    style.visuals.widgets.hovered.fg_stroke.color = Color32::from_rgb(29, 35, 42);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(209, 225, 247);
    style.visuals.widgets.active.fg_stroke.color = Color32::from_rgb(24, 29, 34);
    style.visuals.widgets.open.bg_fill = Color32::from_rgb(220, 232, 250);
    context.set_style(style);
}
