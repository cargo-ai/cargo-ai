use eframe::egui::{self, Color32};

pub fn configure_theme(context: &egui::Context) {
    let mut style = (*context.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.visuals.window_fill = Color32::from_rgb(244, 245, 247);
    style.visuals.panel_fill = Color32::from_rgb(244, 245, 247);
    style.visuals.extreme_bg_color = Color32::from_rgb(246, 247, 248);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(219, 232, 255);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(234, 242, 255);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(238, 239, 241);
    context.set_style(style);
}
