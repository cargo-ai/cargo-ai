use eframe::egui::{self, Color32, RichText, Ui};

use crate::shipyard_ui::config;

pub fn draw(ui: &mut Ui, logo: Option<&egui::TextureHandle>) {
    egui::Frame::new()
        .fill(Color32::from_rgb(248, 248, 249))
        .corner_radius(egui::CornerRadius::same(config::WORKSPACE_CORNER_RADIUS))
        .inner_margin(egui::Margin::same(20))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(70.0);
                if let Some(logo) = logo {
                    let logo_size =
                        scaled_size_for_height(logo.size_vec2(), config::WORKSPACE_LOGO_HEIGHT);
                    ui.add(
                        egui::Image::new(logo)
                            .fit_to_exact_size(logo_size)
                            .tint(Color32::from_white_alpha(config::WORKSPACE_LOGO_ALPHA)),
                    );
                    ui.add_space(14.0);
                }
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

fn scaled_size_for_height(original_size: egui::Vec2, target_height: f32) -> egui::Vec2 {
    if original_size.y <= 0.0 {
        return egui::vec2(target_height, target_height);
    }

    let scale = target_height / original_size.y;
    egui::vec2(original_size.x * scale, target_height)
}
