use eframe::egui;

use crate::shipyard_ui::config;
use crate::shipyard_ui::runtime::events::RunStatus;
use crate::shipyard_ui::widgets::account_onboarding::{AccountSetupAction, AccountSetupState};
use crate::shipyard_ui::widgets::{account_onboarding, execution_feed, title_bar, workspace};

pub struct LayoutResult {
    pub account_setup_action: Option<AccountSetupAction>,
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
    account_setup_state: AccountSetupState,
    account_email_input: &mut String,
    account_code_input: &mut String,
) -> LayoutResult {
    let mut account_setup_action = None;
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
            if result.run_clicked {
                account_setup_action = Some(AccountSetupAction::RunStatus);
            }
            execution_panel_height = Some(ui.max_rect().height());
        });

    egui::CentralPanel::default().show(context, |ui| {
        if matches!(account_setup_state, AccountSetupState::SignedIn) {
            workspace::draw(ui, workspace_logo);
        } else {
            let onboarding_action = account_onboarding::draw(
                ui,
                account_setup_state,
                account_email_input,
                account_code_input,
                status.is_running(),
            );
            if account_setup_action.is_none() {
                account_setup_action = onboarding_action;
            }
        }
    });

    LayoutResult {
        account_setup_action,
        execution_panel_height,
    }
}
