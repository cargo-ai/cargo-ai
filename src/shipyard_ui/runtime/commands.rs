use std::path::PathBuf;

use crate::shipyard_ui::config;

#[derive(Clone, Copy)]
pub enum CommandIntent {
    ProfileList,
}

#[derive(Clone)]
pub struct CommandPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub display: String,
}

pub fn button_label(intent: CommandIntent) -> &'static str {
    match intent {
        CommandIntent::ProfileList => config::PROFILE_LIST_INTENT_LABEL,
    }
}

pub fn command_plan(intent: CommandIntent) -> CommandPlan {
    match intent {
        CommandIntent::ProfileList => profile_list_plan(),
    }
}

fn profile_list_plan() -> CommandPlan {
    let args: Vec<String> = config::PROFILE_LIST_VERBOSE_ARGS
        .iter()
        .map(|value| value.to_string())
        .collect();

    if let Ok(current_exe) = std::env::current_exe() {
        let display = format!("{} {}", current_exe.display(), args.join(" "));
        return CommandPlan {
            program: current_exe,
            args,
            display,
        };
    }

    let fallback_program = PathBuf::from("cargo-ai");
    let display = format!("{} {}", fallback_program.display(), args.join(" "));
    CommandPlan {
        program: fallback_program,
        args,
        display,
    }
}
