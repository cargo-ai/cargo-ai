use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use crate::shipyard_ui::assets::LoadedAssets;
use crate::shipyard_ui::config;
use crate::shipyard_ui::layout;
use crate::shipyard_ui::runtime::commands::{self, CommandIntent};
use crate::shipyard_ui::runtime::events::{RunStatus, StreamKind, TerminalEvent};
use crate::shipyard_ui::runtime::executor;
use crate::shipyard_ui::state;
use crate::shipyard_ui::widgets::account_onboarding::{AccountSetupAction, AccountSetupState};

pub struct ShipyardApp {
    output_lines: Vec<String>,
    current_run_output_lines: Vec<String>,
    status: RunStatus,
    last_command: String,
    active_intent: Option<CommandIntent>,
    auto_started: bool,
    execution_panel_height: Option<f32>,
    last_saved_execution_panel_height: Option<f32>,
    loaded_assets: Option<LoadedAssets>,
    ui_zoom_factor: f32,
    account_setup_state: AccountSetupState,
    account_email_input: String,
    account_code_input: String,
    event_tx: Sender<TerminalEvent>,
    event_rx: Receiver<TerminalEvent>,
}

impl ShipyardApp {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        let loaded_execution_panel_height = state::load_execution_panel_height();

        Self {
            output_lines: Vec::new(),
            current_run_output_lines: Vec::new(),
            status: RunStatus::Idle,
            last_command: String::new(),
            active_intent: None,
            auto_started: false,
            execution_panel_height: loaded_execution_panel_height,
            last_saved_execution_panel_height: loaded_execution_panel_height,
            loaded_assets: None,
            ui_zoom_factor: 1.0,
            account_setup_state: AccountSetupState::Checking,
            account_email_input: String::new(),
            account_code_input: String::new(),
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
                    let formatted = format!("{stream_name} | {line}");
                    self.current_run_output_lines.push(formatted.clone());
                    self.output_lines.push(formatted);
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
                    self.output_lines.push(formatted);

                    if let Some(intent) = self.active_intent.take() {
                        self.handle_intent_finished(intent, false);
                    }
                }
            }
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
                self.account_code_input.clear();
                if success {
                    self.run_intent(CommandIntent::AccountStatus);
                } else {
                    self.account_setup_state = AccountSetupState::Unknown;
                }
            }
        }
    }

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

impl eframe::App for ShipyardApp {
    fn update(&mut self, context: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        self.flush_events();
        self.apply_zoom_for_display(context);

        let viewport_height = context.screen_rect().height();
        let panel_max_height = config::execution_panel_max_height(viewport_height);
        let panel_default_height = self
            .execution_panel_height
            .unwrap_or_else(|| config::execution_panel_default_height(viewport_height));
        let panel_default_height = clamp_panel_height(panel_default_height, viewport_height);

        if !self.auto_started {
            self.auto_started = true;
            self.run_intent(CommandIntent::AccountStatus);
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
            commands::button_label(&CommandIntent::AccountStatus),
            &self.last_command,
            &self.output_lines,
            panel_default_height,
            panel_max_height,
            loaded_assets.logo_color.as_ref(),
            loaded_assets.logo_bw.as_ref(),
            self.account_setup_state,
            &mut self.account_email_input,
            &mut self.account_code_input,
        );

        if let Some(observed_height) = result.execution_panel_height {
            let clamped_height = clamp_panel_height(observed_height, viewport_height);
            self.execution_panel_height = Some(clamped_height);
            self.persist_panel_height_if_stable(context, clamped_height);
        }

        if let Some(action) = result.account_setup_action {
            match action {
                AccountSetupAction::RunStatus => self.run_intent(CommandIntent::AccountStatus),
                AccountSetupAction::Register { email } => {
                    self.run_intent(CommandIntent::AccountRegister { email })
                }
                AccountSetupAction::Confirm { code } => {
                    self.run_intent(CommandIntent::AccountConfirm { code })
                }
            }
        }
    }
}

impl ShipyardApp {
    fn apply_zoom_for_display(&mut self, context: &eframe::egui::Context) {
        let pixels_per_point = context.pixels_per_point();
        let target_zoom = if pixels_per_point < config::LOW_DPI_PPP_THRESHOLD {
            config::LOW_DPI_ZOOM_FACTOR
        } else {
            1.0
        };

        if (self.ui_zoom_factor - target_zoom).abs() >= 0.01 {
            context.set_zoom_factor(target_zoom);
            self.ui_zoom_factor = target_zoom;
        }
    }
}

fn clamp_panel_height(height: f32, viewport_height: f32) -> f32 {
    height
        .max(config::EXECUTION_PANEL_MIN_HEIGHT)
        .min(config::execution_panel_max_height(viewport_height))
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
