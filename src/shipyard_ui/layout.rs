use eframe::egui;

use crate::shipyard_ui::config;
use crate::shipyard_ui::runtime::events::RunStatus;
use crate::shipyard_ui::widgets::{execution_feed, title_bar, workspace};

pub struct LayoutResult {
    pub run_default_intent: bool,
    pub execution_panel_height: Option<f32>,
}

pub fn draw(
    context: &egui::Context,
    status: &RunStatus,
    command_label: &str,
    last_command: &str,
    output_lines: &[String],
    execution_panel_default_height: f32,
    execution_panel_max_height: f32,
    title_logo: Option<&egui::TextureHandle>,
    workspace_logo: Option<&egui::TextureHandle>,
) -> LayoutResult {
    let mut run_default_intent = false;
    let mut execution_panel_height = None;

    egui::TopBottomPanel::top("shipyard_title")
        .exact_height(config::TITLE_PANEL_HEIGHT)
        .show(context, |ui| {
            title_bar::draw(ui, title_logo);
        });

    egui::TopBottomPanel::bottom("shipyard_execution")
        .default_height(execution_panel_default_height)
        .min_height(config::EXECUTION_PANEL_MIN_HEIGHT)
        .max_height(execution_panel_max_height)
        .resizable(true)
        .show(context, |ui| {
            let result =
                execution_feed::draw(ui, status, command_label, last_command, output_lines);
            run_default_intent = result.run_clicked;
            execution_panel_height = Some(ui.max_rect().height());
        });

    egui::CentralPanel::default().show(context, |ui| {
        workspace::draw(ui, workspace_logo);
    });

    LayoutResult {
        run_default_intent,
        execution_panel_height,
    }
}
