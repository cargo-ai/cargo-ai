use eframe::egui::{self, Color32, RichText, Ui};

use crate::shipyard_ui::config;
use crate::shipyard_ui::runtime::events::RunStatus;

pub struct ExecutionFeedResult {
    pub run_clicked: bool,
}

pub fn draw(
    ui: &mut Ui,
    status: &RunStatus,
    command_label: &str,
    last_command: &str,
    output_lines: &[String],
) -> ExecutionFeedResult {
    let mut result = ExecutionFeedResult { run_clicked: false };

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Execution Feed")
                .strong()
                .size(15.0)
                .color(Color32::from_rgb(36, 40, 45)),
        );

        let (status_label, status_color, status_code) = status_badge(status);
        ui.label(RichText::new(status_label).color(status_color).strong());
        if let Some(code) = status_code {
            ui.label(
                RichText::new(format!("(exit {code})"))
                    .size(12.0)
                    .color(Color32::from_rgb(106, 112, 120)),
            );
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(
                    !status.is_running(),
                    egui::Button::new(command_label.to_string()),
                )
                .clicked()
            {
                result.run_clicked = true;
            }
        });
    });

    if !last_command.is_empty() {
        ui.label(
            RichText::new(format!("Command: {last_command}"))
                .monospace()
                .color(Color32::from_rgb(85, 92, 101)),
        );
    }

    egui::Frame::new()
        .fill(Color32::from_rgb(22, 24, 28))
        .corner_radius(egui::CornerRadius::same(config::TERMINAL_CORNER_RADIUS))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if output_lines.is_empty() {
                        ui.label(
                            RichText::new("No command output yet.")
                                .monospace()
                                .color(Color32::from_rgb(147, 152, 160)),
                        );
                        return;
                    }

                    for line in output_lines {
                        let text_color = if line.starts_with("stderr |") {
                            Color32::from_rgb(255, 158, 158)
                        } else {
                            Color32::from_rgb(211, 218, 227)
                        };
                        ui.label(
                            RichText::new(line)
                                .monospace()
                                .size(config::TERMINAL_FONT_SIZE)
                                .color(text_color),
                        );
                    }
                });
        });

    if let RunStatus::SpawnError(message) = status {
        ui.label(RichText::new(format!("Error: {message}")).color(Color32::from_rgb(199, 57, 57)));
    }

    result
}

fn status_badge(status: &RunStatus) -> (&'static str, Color32, Option<i32>) {
    match status {
        RunStatus::Idle => ("idle", Color32::from_rgb(120, 128, 136), None),
        RunStatus::Running => ("running", Color32::from_rgb(41, 201, 67), None),
        RunStatus::Succeeded(code) => ("success", Color32::from_rgb(54, 156, 86), *code),
        RunStatus::Failed(code) => ("failed", Color32::from_rgb(199, 57, 57), *code),
        RunStatus::SpawnError(_) => ("spawn error", Color32::from_rgb(199, 57, 57), None),
    }
}
