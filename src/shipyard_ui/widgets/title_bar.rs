use eframe::egui::{self, Color32, RichText, Ui};

use crate::shipyard_ui::config;

pub fn draw(ui: &mut Ui, logo: Option<&egui::TextureHandle>) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("●")
                .size(16.0)
                .color(Color32::from_rgb(251, 95, 87)),
        );
        ui.label(
            RichText::new("●")
                .size(16.0)
                .color(Color32::from_rgb(252, 191, 63)),
        );
        ui.label(
            RichText::new("●")
                .size(16.0)
                .color(Color32::from_rgb(41, 201, 67)),
        );
        ui.add_space(12.0);
        if let Some(logo) = logo {
            let logo_size = scaled_size_for_height(logo.size_vec2(), config::TITLE_LOGO_HEIGHT);
            ui.add(egui::Image::new(logo).fit_to_exact_size(logo_size));
            ui.add_space(8.0);
        }
        ui.label(
            RichText::new("Shipyard")
                .size(18.0)
                .color(Color32::from_rgb(34, 38, 43))
                .strong(),
        );
        ui.label(
            RichText::new("Phase 1")
                .size(13.0)
                .color(Color32::from_rgb(106, 112, 120)),
        );
    });
}

fn scaled_size_for_height(original_size: egui::Vec2, target_height: f32) -> egui::Vec2 {
    if original_size.y <= 0.0 {
        return egui::vec2(target_height, target_height);
    }

    let scale = target_height / original_size.y;
    egui::vec2(original_size.x * scale, target_height)
}
