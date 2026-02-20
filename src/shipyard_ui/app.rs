use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use slint::{ComponentHandle, Timer};

use crate::shipyard_ui::config;
use crate::shipyard_ui::runtime::commands::{self, CommandIntent};
use crate::shipyard_ui::runtime::events::{RunStatus, StreamKind, TerminalEvent};
use crate::shipyard_ui::runtime::executor;
use crate::shipyard_ui::state;
use crate::shipyard_ui::ui;

#[derive(Clone, Copy)]
enum AccountSetupState {
    Checking,
    NeedsSetup,
    SignedIn,
    Unknown,
}

impl AccountSetupState {
    fn status_text(self) -> &'static str {
        match self {
            Self::Checking => "Checking account status...",
            Self::NeedsSetup => "Account setup required (register, confirm, then status).",
            Self::SignedIn => "Authenticated. Account is ready.",
            Self::Unknown => "Status uncertain. Run account status to verify current session.",
        }
    }
}

pub fn launch() -> Result<(), String> {
    let window = ui::ShipyardWindow::new().map_err(|error| error.to_string())?;
    let (event_tx, event_rx) = mpsc::channel();

    let controller = Rc::new(RefCell::new(ShipyardController::new(
        window.as_weak(),
        event_tx,
        event_rx,
    )));

    wire_callbacks(&window, &controller);

    controller.borrow_mut().sync_ui();
    controller
        .borrow_mut()
        .run_intent(CommandIntent::AccountStatus);

    let poll_timer = Timer::default();
    let poll_controller = controller.clone();
    poll_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(config::REPAINT_INTERVAL_MS),
        move || {
            poll_controller.borrow_mut().flush_events();
        },
    );

    window.run().map_err(|error| error.to_string())
}

fn wire_callbacks(window: &ui::ShipyardWindow, controller: &Rc<RefCell<ShipyardController>>) {
    let status_controller = controller.clone();
    window.on_run_status(move || {
        status_controller
            .borrow_mut()
            .run_intent(CommandIntent::AccountStatus);
    });

    let register_controller = controller.clone();
    window.on_run_register(move |email| {
        let email = email.trim();
        if email.is_empty() {
            return;
        }
        register_controller
            .borrow_mut()
            .run_intent(CommandIntent::AccountRegister {
                email: email.to_string(),
            });
    });

    let confirm_controller = controller.clone();
    window.on_run_confirm(move |code| {
        let code = code.trim();
        if code.is_empty() {
            return;
        }
        confirm_controller
            .borrow_mut()
            .run_intent(CommandIntent::AccountConfirm {
                code: code.to_string(),
            });
    });

    let ratio_controller = controller.clone();
    window.on_execution_panel_ratio_changed(move |ratio| {
        ratio_controller
            .borrow_mut()
            .set_execution_panel_ratio(ratio);
    });
}

struct ShipyardController {
    window: slint::Weak<ui::ShipyardWindow>,
    output_lines: Vec<String>,
    current_run_output_lines: Vec<String>,
    status: RunStatus,
    last_command: String,
    active_intent: Option<CommandIntent>,
    account_setup_state: AccountSetupState,
    execution_panel_ratio: f32,
    event_tx: Sender<TerminalEvent>,
    event_rx: Receiver<TerminalEvent>,
}

impl ShipyardController {
    fn new(
        window: slint::Weak<ui::ShipyardWindow>,
        event_tx: Sender<TerminalEvent>,
        event_rx: Receiver<TerminalEvent>,
    ) -> Self {
        Self {
            window,
            output_lines: Vec::new(),
            current_run_output_lines: Vec::new(),
            status: RunStatus::Idle,
            last_command: String::new(),
            active_intent: None,
            account_setup_state: AccountSetupState::Checking,
            execution_panel_ratio: state::load_execution_panel_ratio()
                .unwrap_or(config::EXECUTION_PANEL_DEFAULT_RATIO),
            event_tx,
            event_rx,
        }
    }

    fn run_intent(&mut self, intent: CommandIntent) {
        if self.status.is_running() {
            return;
        }

        let plan = commands::command_plan(&intent);
        self.last_command = plan.display.clone();
        self.status = RunStatus::Running;
        self.current_run_output_lines.clear();
        self.active_intent = Some(intent);
        self.push_output_line(format!("$ {}", self.last_command));

        executor::spawn_command(plan, self.event_tx.clone());
        self.sync_ui();
    }

    fn flush_events(&mut self) {
        let mut saw_update = false;

        while let Ok(event) = self.event_rx.try_recv() {
            saw_update = true;
            match event {
                TerminalEvent::Output { stream, line } => {
                    let stream_name = match stream {
                        StreamKind::Stdout => "stdout",
                        StreamKind::Stderr => "stderr",
                    };
                    let formatted = format!("{stream_name} | {line}");
                    self.current_run_output_lines.push(formatted.clone());
                    self.push_output_line(formatted);
                }
                TerminalEvent::Finished { success, code } => {
                    self.status = if success {
                        RunStatus::Succeeded(code)
                    } else {
                        RunStatus::Failed(code)
                    };

                    if let Some(intent) = self.active_intent.take() {
                        self.handle_intent_finished(intent, success);
                    }
                }
                TerminalEvent::SpawnFailed(message) => {
                    self.status = RunStatus::SpawnError(message.clone());
                    let formatted = format!("stderr | {message}");
                    self.current_run_output_lines.push(formatted.clone());
                    self.push_output_line(formatted);

                    if let Some(intent) = self.active_intent.take() {
                        self.handle_intent_finished(intent, false);
                    }
                }
            }
        }

        if saw_update {
            self.sync_ui();
        }
    }

    fn handle_intent_finished(&mut self, intent: CommandIntent, success: bool) {
        match intent {
            CommandIntent::AccountStatus => {
                self.account_setup_state =
                    derive_account_setup_state(&self.current_run_output_lines, success);
            }
            CommandIntent::AccountRegister { .. } => {
                self.account_setup_state = AccountSetupState::NeedsSetup;
                if success {
                    self.run_intent(CommandIntent::AccountStatus);
                }
            }
            CommandIntent::AccountConfirm { .. } => {
                if let Some(window) = self.window.upgrade() {
                    window.set_account_code("".into());
                }

                if success {
                    self.run_intent(CommandIntent::AccountStatus);
                } else {
                    self.account_setup_state = AccountSetupState::Unknown;
                }
            }
        }
    }

    fn set_execution_panel_ratio(&mut self, ratio: f32) {
        let clamped = config::clamp_execution_panel_ratio(ratio);
        if (self.execution_panel_ratio - clamped).abs() < 0.0005 {
            return;
        }

        self.execution_panel_ratio = clamped;
        let _ = state::save_execution_panel_ratio(clamped);
        self.sync_ui();
    }

    fn sync_ui(&mut self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };

        let (status_label, status_kind) = status_view(&self.status);

        window.set_status_label(status_label.into());
        window.set_status_kind(status_kind);
        window.set_command_running(self.status.is_running());
        window.set_last_command(self.last_command.clone().into());
        window.set_account_status_text(self.account_setup_state.status_text().into());
        window.set_account_ready(matches!(
            self.account_setup_state,
            AccountSetupState::SignedIn
        ));
        window.set_terminal_output(self.output_lines.join("\n").into());
        window.set_execution_panel_ratio(self.execution_panel_ratio);
    }

    fn push_output_line(&mut self, line: String) {
        self.output_lines.push(line);
        if self.output_lines.len() > config::MAX_TERMINAL_LINES {
            let overflow = self.output_lines.len() - config::MAX_TERMINAL_LINES;
            self.output_lines.drain(0..overflow);
        }
    }
}

fn status_view(status: &RunStatus) -> (String, i32) {
    match status {
        RunStatus::Idle => ("idle".to_string(), 0),
        RunStatus::Running => ("running".to_string(), 1),
        RunStatus::Succeeded(code) => {
            if let Some(code) = code {
                (format!("success (exit {code})"), 2)
            } else {
                ("success".to_string(), 2)
            }
        }
        RunStatus::Failed(code) => {
            if let Some(code) = code {
                (format!("failed (exit {code})"), 3)
            } else {
                ("failed".to_string(), 3)
            }
        }
        RunStatus::SpawnError(message) => (format!("spawn error: {message}"), 4),
    }
}

fn derive_account_setup_state(lines: &[String], success: bool) -> AccountSetupState {
    if !success {
        return AccountSetupState::Unknown;
    }

    let output = lines.join("\n").to_lowercase();

    let setup_missing_markers = [
        "no local config file found",
        "no account found in config",
        "no access token found in config",
        "you must confirm your account first",
        "run `cargo ai account register <email>` first",
    ];

    if setup_missing_markers
        .iter()
        .any(|marker| output.contains(marker))
    {
        return AccountSetupState::NeedsSetup;
    }

    if output.contains("request failed") || output.contains("spawn error") {
        return AccountSetupState::Unknown;
    }

    AccountSetupState::SignedIn
}
