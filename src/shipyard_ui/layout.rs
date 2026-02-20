use eframe::egui;

use crate::shipyard_ui::config;
use crate::shipyard_ui::runtime::events::RunStatus;
use crate::shipyard_ui::widgets::{execution_feed, title_bar, workspace};

pub struct LayoutResult {
    pub run_default_intent: bool,
}

pub fn draw(
    context: &egui::Context,
    status: &RunStatus,
    command_label: &str,
    last_command: &str,
    output_lines: &[String],
) -> LayoutResult {
    let mut run_default_intent = false;

    egui::TopBottomPanel::top("shipyard_title")
        .exact_height(config::TITLE_PANEL_HEIGHT)
        .show(context, |ui| {
            title_bar::draw(ui);
        });

    egui::TopBottomPanel::bottom("shipyard_execution")
        .default_height(config::EXECUTION_PANEL_DEFAULT_HEIGHT)
        .min_height(config::EXECUTION_PANEL_MIN_HEIGHT)
        .resizable(true)
        .show(context, |ui| {
            let panel_height = ui
                .available_height()
                .max(config::EXECUTION_PANEL_MIN_HEIGHT);
            let target_height = panel_height * config::EXECUTION_PANEL_TARGET_RATIO;
            ui.set_min_height(target_height.max(config::EXECUTION_PANEL_MIN_HEIGHT));

            let result =
                execution_feed::draw(ui, status, command_label, last_command, output_lines);
            run_default_intent = result.run_clicked;
        });

    egui::CentralPanel::default().show(context, |ui| {
        workspace::draw(ui);
    });

    LayoutResult { run_default_intent }
}
