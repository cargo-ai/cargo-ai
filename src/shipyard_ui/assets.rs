use eframe::egui;

pub struct LoadedAssets {
    pub logo_color: Option<egui::TextureHandle>,
    pub logo_bw: Option<egui::TextureHandle>,
}

impl LoadedAssets {
    pub fn load(context: &egui::Context) -> Self {
        Self {
            logo_color: load_texture(
                context,
                "shipyard_logo_color",
                include_bytes!("assets/cai-logo-2.png"),
            ),
            logo_bw: load_texture(
                context,
                "shipyard_logo_bw",
                include_bytes!("assets/cai-logo-2-bw.png"),
            ),
        }
    }
}

fn load_texture(context: &egui::Context, name: &str, bytes: &[u8]) -> Option<egui::TextureHandle> {
    let image = image::load_from_memory(bytes).ok()?;
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let pixels = rgba.into_raw();
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
    Some(context.load_texture(name, color_image, egui::TextureOptions::LINEAR))
}
