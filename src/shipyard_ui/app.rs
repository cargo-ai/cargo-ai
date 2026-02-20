use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use crate::shipyard_ui::assets::LoadedAssets;
use crate::shipyard_ui::config;
use crate::shipyard_ui::layout;
use crate::shipyard_ui::runtime::commands::{self, CommandIntent};
use crate::shipyard_ui::runtime::events::{RunStatus, StreamKind, TerminalEvent};
use crate::shipyard_ui::runtime::executor;
use crate::shipyard_ui::state;

pub struct ShipyardApp {
    output_lines: Vec<String>,
    status: RunStatus,
    last_command: String,
    auto_started: bool,
    execution_panel_height: Option<f32>,
    last_saved_execution_panel_height: Option<f32>,
    loaded_assets: Option<LoadedAssets>,
    event_tx: Sender<TerminalEvent>,
    event_rx: Receiver<TerminalEvent>,
}

impl ShipyardApp {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        let loaded_execution_panel_height = state::load_execution_panel_height();

        Self {
            output_lines: Vec::new(),
            status: RunStatus::Idle,
            last_command: String::new(),
            auto_started: false,
            execution_panel_height: loaded_execution_panel_height,
            last_saved_execution_panel_height: loaded_execution_panel_height,
            loaded_assets: None,
            event_tx,
            event_rx,
        }
    }

    fn run_intent(&mut self, intent: CommandIntent) {
        if self.status.is_running() {
            return;
        }

        let plan = commands::command_plan(intent);
        self.last_command = plan.display.clone();
        self.status = RunStatus::Running;
        self.output_lines.push(format!("$ {}", self.last_command));

        executor::spawn_command(plan, self.event_tx.clone());
    }

    fn flush_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                TerminalEvent::Output { stream, line } => {
                    let stream_name = match stream {
                        StreamKind::Stdout => "stdout",
                        StreamKind::Stderr => "stderr",
                    };
                    self.output_lines.push(format!("{stream_name} | {line}"));
                }
                TerminalEvent::Finished { success, code } => {
                    self.status = if success {
                        RunStatus::Succeeded(code)
                    } else {
                        RunStatus::Failed(code)
                    };
                }
                TerminalEvent::SpawnFailed(message) => {
                    self.status = RunStatus::SpawnError(message.clone());
                    self.output_lines.push(format!("stderr | {message}"));
                }
            }
        }
    }
}

impl eframe::App for ShipyardApp {
    fn update(&mut self, context: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        self.flush_events();

        let viewport_height = context.screen_rect().height();
        let panel_max_height = config::execution_panel_max_height(viewport_height);
        let panel_default_height = self
            .execution_panel_height
            .unwrap_or_else(|| config::execution_panel_default_height(viewport_height));
        let panel_default_height = clamp_panel_height(panel_default_height, viewport_height);

        if !self.auto_started {
            self.auto_started = true;
            self.run_intent(CommandIntent::ProfileList);
        }

        if self.status.is_running() {
            context.request_repaint_after(Duration::from_millis(config::REPAINT_INTERVAL_MS));
        }

        let loaded_assets = self
            .loaded_assets
            .get_or_insert_with(|| LoadedAssets::load(context));

        let result = layout::draw(
            context,
            &self.status,
            commands::button_label(CommandIntent::ProfileList),
            &self.last_command,
            &self.output_lines,
            panel_default_height,
            panel_max_height,
            loaded_assets.logo_color.as_ref(),
            loaded_assets.logo_bw.as_ref(),
        );

        if let Some(observed_height) = result.execution_panel_height {
            let clamped_height = clamp_panel_height(observed_height, viewport_height);
            self.execution_panel_height = Some(clamped_height);
            self.persist_panel_height_if_stable(context, clamped_height);
        }

        if result.run_default_intent {
            self.run_intent(CommandIntent::ProfileList);
        }
    }
}

impl ShipyardApp {
    fn persist_panel_height_if_stable(
        &mut self,
        context: &eframe::egui::Context,
        execution_panel_height: f32,
    ) {
        let is_pointer_down = context.input(|input_state| input_state.pointer.any_down());
        if is_pointer_down {
            return;
        }

        let should_write = self
            .last_saved_execution_panel_height
            .map(|saved| {
                (saved - execution_panel_height).abs()
                    >= config::EXECUTION_PANEL_PERSIST_WRITE_THRESHOLD
            })
            .unwrap_or(true);

        if should_write && state::save_execution_panel_height(execution_panel_height).is_ok() {
            self.last_saved_execution_panel_height = Some(execution_panel_height);
        }
    }
}

fn clamp_panel_height(height: f32, viewport_height: f32) -> f32 {
    height
        .max(config::EXECUTION_PANEL_MIN_HEIGHT)
        .min(config::execution_panel_max_height(viewport_height))
}
