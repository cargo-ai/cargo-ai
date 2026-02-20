use eframe::egui::{Color32, RichText, Ui};

pub fn draw(ui: &mut Ui) {
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
