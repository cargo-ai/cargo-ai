use eframe::egui::{self, Color32, RichText, Ui};

use crate::shipyard_ui::config;

pub fn draw(ui: &mut Ui) {
    egui::Frame::new()
        .fill(Color32::from_rgb(248, 248, 249))
        .corner_radius(egui::CornerRadius::same(config::WORKSPACE_CORNER_RADIUS))
        .inner_margin(egui::Margin::same(20))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(70.0);
                ui.label(
                    RichText::new("Shipyard Workspace")
                        .size(26.0)
                        .color(Color32::from_rgb(43, 47, 54))
                        .strong(),
                );
                ui.label(
                    RichText::new("Primary interaction area (Phase 1 placeholder)")
                        .size(14.0)
                        .color(Color32::from_rgb(110, 116, 124)),
                );
                ui.add_space(14.0);
            });
            ui.allocate_space(ui.available_size());
        });
}
