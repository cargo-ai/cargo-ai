//! Action execution helpers for preflight/test flows.
use jsonlogic::apply;

/// Applies configured action rules to model output and executes matching steps.
pub(crate) fn apply_actions(output: &crate::Output, actions: &[crate::Action]) {
    // println!("DEBUG: Applying actions -> {:?}", actions);

    let data = match serde_json::to_value(output) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("❌ Failed to serialize output for action evaluation: {error}");
            return;
        }
    };
    let current_platform = current_action_platform();

    for action in actions {
        match apply(&action.logic, &data) {
            Ok(result) => {
                // println!("Action Loop: {:?}", action);
                if result.as_bool() == Some(true) {
                    let matching_steps = matching_run_steps(&action.run, current_platform);
                    if matching_steps.is_empty() {
                        eprintln!(
                            "⚠️ No run steps matched the current platform for action '{}' (current platform: {}).",
                            action.name,
                            current_platform.unwrap_or("unsupported")
                        );
                        continue;
                    }

                    for step in matching_steps {
                        if !step.kind.eq_ignore_ascii_case("exec") {
                            eprintln!(
                                "⚠️ Skipping action '{}' with unsupported step kind '{}'.",
                                action.name, step.kind
                            );
                            continue;
                        }

                        println!(
                            "Running '{}': {} {:?}",
                            action.name, step.program, step.args
                        );

                        // Execute the command
                        let status = std::process::Command::new(&step.program)
                            .args(&step.args)
                            .status();

                        match status {
                            Ok(status) if status.success() => {
                                println!("Command completed successfully.");
                            }
                            Ok(status) => {
                                println!("Command exited with status: {}", status);
                            }
                            Err(err) => {
                                println!("Failed to execute command: {}", err);
                            }
                        }
                    }
                }
            }
            Err(error) => {
                println!(
                    "Failed to evaluate logic for action '{}': {}",
                    action.name, error
                );
            }
        }
    }
}

fn current_action_platform() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("macos")
    } else if cfg!(target_os = "linux") {
        Some("linux")
    } else if cfg!(target_os = "windows") {
        Some("windows")
    } else {
        None
    }
}

fn matching_run_steps<'a>(
    run_steps: &'a [crate::RunStep],
    current_platform: Option<&str>,
) -> Vec<&'a crate::RunStep> {
    run_steps
        .iter()
        .filter(|step| step_matches_platform(step.platforms.as_deref(), current_platform))
        .collect()
}

fn step_matches_platform(platforms: Option<&[String]>, current_platform: Option<&str>) -> bool {
    match platforms {
        None => true,
        Some(platforms) => current_platform.is_some_and(|platform| {
            platforms.iter().any(|candidate| candidate == platform)
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{matching_run_steps, step_matches_platform};

    fn run_step(program: &str, platforms: Option<&[&str]>) -> crate::RunStep {
        crate::RunStep {
            kind: "exec".to_string(),
            program: program.to_string(),
            args: vec![],
            platforms: platforms.map(|platforms| {
                platforms.iter().map(|platform| platform.to_string()).collect()
            }),
        }
    }

    #[test]
    fn platformless_steps_match_supported_platforms() {
        assert!(step_matches_platform(None, Some("macos")));
        assert!(step_matches_platform(None, Some("linux")));
        assert!(step_matches_platform(None, None));
    }

    #[test]
    fn explicit_platforms_match_only_listed_platforms() {
        let platforms = vec!["macos".to_string(), "linux".to_string()];
        assert!(step_matches_platform(Some(&platforms), Some("macos")));
        assert!(step_matches_platform(Some(&platforms), Some("linux")));
        assert!(!step_matches_platform(Some(&platforms), Some("windows")));
        assert!(!step_matches_platform(Some(&platforms), None));
    }

    #[test]
    fn matching_run_steps_preserve_declared_order() {
        let run_steps = vec![
            run_step("first", Some(&["windows"])),
            run_step("second", None),
            run_step("third", Some(&["macos", "linux"])),
            run_step("fourth", None),
        ];

        let matching = matching_run_steps(&run_steps, Some("macos"));
        let programs = matching
            .iter()
            .map(|step| step.program.as_str())
            .collect::<Vec<_>>();

        assert_eq!(programs, vec!["second", "third", "fourth"]);
    }
}
