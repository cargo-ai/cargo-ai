use std::path::PathBuf;

use crate::shipyard_ui::config;

#[derive(Clone)]
pub enum CommandIntent {
    AccountStatus,
    AccountRegister { email: String },
    AccountConfirm { code: String },
}

#[derive(Clone)]
pub struct CommandPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub display: String,
}

pub fn command_plan(intent: &CommandIntent) -> CommandPlan {
    match intent {
        CommandIntent::AccountStatus => account_status_plan(),
        CommandIntent::AccountRegister { email } => account_register_plan(email),
        CommandIntent::AccountConfirm { code } => account_confirm_plan(code),
    }
}

fn account_status_plan() -> CommandPlan {
    let args: Vec<String> = config::ACCOUNT_STATUS_VERBOSE_ARGS
        .iter()
        .map(|value| value.to_string())
        .collect();
    plan_with_args(args, None)
}

fn account_register_plan(email: &str) -> CommandPlan {
    let mut args: Vec<String> = config::ACCOUNT_REGISTER_VERBOSE_PREFIX_ARGS
        .iter()
        .map(|value| value.to_string())
        .collect();
    args.push(email.to_string());
    plan_with_args(args, None)
}

fn account_confirm_plan(code: &str) -> CommandPlan {
    let mut args: Vec<String> = config::ACCOUNT_CONFIRM_VERBOSE_PREFIX_ARGS
        .iter()
        .map(|value| value.to_string())
        .collect();
    args.push(code.to_string());
    plan_with_args(args, Some("<redacted-code>".to_string()))
}

fn plan_with_args(args: Vec<String>, redacted_last_arg: Option<String>) -> CommandPlan {
    if let Ok(current_exe) = std::env::current_exe() {
        let display_args = display_args(&args, redacted_last_arg);
        let display = format!("{} {}", current_exe.display(), display_args.join(" "));
        return CommandPlan {
            program: current_exe,
            args,
            display,
        };
    }

    let fallback_program = PathBuf::from("cargo-ai");
    let display_args = display_args(&args, redacted_last_arg);
    let display = format!("{} {}", fallback_program.display(), display_args.join(" "));
    CommandPlan {
        program: fallback_program,
        args,
        display,
    }
}

fn display_args(args: &[String], redacted_last_arg: Option<String>) -> Vec<String> {
    let mut values = args.to_vec();
    if let Some(replacement) = redacted_last_arg {
        if let Some(last) = values.last_mut() {
            *last = replacement;
        }
    }
    values
}
