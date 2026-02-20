use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use crate::shipyard_ui::config;
use crate::shipyard_ui::layout;
use crate::shipyard_ui::runtime::commands::{self, CommandIntent};
use crate::shipyard_ui::runtime::events::{RunStatus, StreamKind, TerminalEvent};
use crate::shipyard_ui::runtime::executor;

pub struct ShipyardApp {
    output_lines: Vec<String>,
    status: RunStatus,
    last_command: String,
    auto_started: bool,
    event_tx: Sender<TerminalEvent>,
    event_rx: Receiver<TerminalEvent>,
}

impl ShipyardApp {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel();

        Self {
            output_lines: Vec::new(),
            status: RunStatus::Idle,
            last_command: String::new(),
            auto_started: false,
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

        if !self.auto_started {
            self.auto_started = true;
            self.run_intent(CommandIntent::ProfileList);
        }

        if self.status.is_running() {
            context.request_repaint_after(Duration::from_millis(config::REPAINT_INTERVAL_MS));
        }

        let result = layout::draw(
            context,
            &self.status,
            commands::button_label(CommandIntent::ProfileList),
            &self.last_command,
            &self.output_lines,
        );

        if result.run_default_intent {
            self.run_intent(CommandIntent::ProfileList);
        }
    }
}
