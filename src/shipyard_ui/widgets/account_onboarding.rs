use eframe::egui::{self, Color32, FontFamily, FontId, RichText, TextStyle, Ui};

use crate::shipyard_ui::config;

#[derive(Clone, Copy)]
pub enum AccountSetupState {
    Checking,
    NeedsSetup,
    SignedIn,
    Unknown,
}

pub enum AccountSetupAction {
    RunStatus,
    Register { email: String },
    Confirm { code: String },
}

pub fn draw(
    ui: &mut Ui,
    account_state: AccountSetupState,
    email_input: &mut String,
    code_input: &mut String,
    is_command_running: bool,
) -> Option<AccountSetupAction> {
    let mut action = None;
    let onboarding_input_style = TextStyle::Name("shipyard_onboarding_input".into());
    let mut scoped_style = ui.style().as_ref().clone();
    scoped_style.text_styles.insert(
        onboarding_input_style.clone(),
        FontId::new(config::ONBOARDING_INPUT_FONT_SIZE, FontFamily::Monospace),
    );
    ui.set_style(scoped_style);

    egui::Frame::new()
        .fill(Color32::from_rgb(248, 248, 249))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(20))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new("Account Setup")
                        .size(config::ONBOARDING_TITLE_FONT_SIZE)
                        .color(Color32::from_rgb(43, 47, 54))
                        .strong(),
                );

                let (status_text, status_color) = status_badge(account_state);
                ui.label(
                    RichText::new(status_text)
                        .size(config::ONBOARDING_STATUS_FONT_SIZE)
                        .color(status_color)
                        .strong(),
                );
                ui.add_space(4.0);

                ui.label(
                    RichText::new(
                        "Set up your Cargo AI account directly from Shipyard using explicit CLI flows.",
                    )
                    .size(config::ONBOARDING_SUBTITLE_FONT_SIZE)
                    .color(Color32::from_rgb(86, 93, 103)),
                );
                ui.add_space(14.0);

                ui.separator();
                ui.add_space(10.0);

                ui.label(
                    RichText::new("1) Register Email")
                        .size(config::ONBOARDING_SECTION_FONT_SIZE)
                        .strong(),
                );
                ui.add(
                    egui::TextEdit::singleline(email_input)
                        .font(onboarding_input_style.clone())
                        .hint_text("you@example.com")
                        .desired_width(f32::INFINITY),
                );
                if ui
                    .add_enabled(
                        !is_command_running,
                        egui::Button::new(
                            RichText::new("Run account register")
                                .size(config::ONBOARDING_BUTTON_FONT_SIZE)
                                .color(Color32::from_rgb(42, 47, 54)),
                        ),
                    )
                    .clicked()
                {
                    let email = email_input.trim();
                    if !email.is_empty() {
                        action = Some(AccountSetupAction::Register {
                            email: email.to_string(),
                        });
                    }
                }

                ui.add_space(14.0);

                ui.label(
                    RichText::new("2) Confirm Code")
                        .size(config::ONBOARDING_SECTION_FONT_SIZE)
                        .strong(),
                );
                ui.add(
                    egui::TextEdit::singleline(code_input)
                        .font(onboarding_input_style.clone())
                        .hint_text("temporary code from email")
                        .desired_width(f32::INFINITY),
                );
                if ui
                    .add_enabled(
                        !is_command_running,
                        egui::Button::new(
                            RichText::new("Run account confirm")
                                .size(config::ONBOARDING_BUTTON_FONT_SIZE)
                                .color(Color32::from_rgb(42, 47, 54)),
                        ),
                    )
                    .clicked()
                {
                    let code = code_input.trim();
                    if !code.is_empty() {
                        action = Some(AccountSetupAction::Confirm {
                            code: code.to_string(),
                        });
                        code_input.clear();
                    }
                }

                ui.add_space(14.0);
                ui.label(
                    RichText::new("3) Verify Session")
                        .size(config::ONBOARDING_SECTION_FONT_SIZE)
                        .strong(),
                );
                if ui
                    .add_enabled(
                        !is_command_running,
                        egui::Button::new(
                            RichText::new("Run account status")
                                .size(config::ONBOARDING_BUTTON_FONT_SIZE)
                                .color(Color32::from_rgb(42, 47, 54)),
                        ),
                    )
                    .clicked()
                {
                    action = Some(AccountSetupAction::RunStatus);
                }
            });
        });

    action
}

fn status_badge(state: AccountSetupState) -> (&'static str, Color32) {
    match state {
        AccountSetupState::Checking => {
            ("Checking account status...", Color32::from_rgb(41, 201, 67))
        }
        AccountSetupState::NeedsSetup => (
            "Account setup required (register, confirm, then status).",
            Color32::from_rgb(199, 57, 57),
        ),
        AccountSetupState::SignedIn => (
            "Authenticated. Account is ready.",
            Color32::from_rgb(54, 156, 86),
        ),
        AccountSetupState::Unknown => (
            "Status uncertain. Run account status to verify current session.",
            Color32::from_rgb(208, 125, 29),
        ),
    }
}
