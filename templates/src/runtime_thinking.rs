//! Selection precedence shared by interpreted and generated action runtimes.
use super::*;
use crate::providers::thinking::{self, ThinkingOutcome, ThinkingSetting, ThinkingSupport};
use crate::providers::thinking_metadata::{request_support, ThinkingRequestKind};

pub(crate) fn invocation_setting(matches: &clap::ArgMatches) -> Option<ThinkingSetting> {
    if matches.get_flag("thinking_provider_default") {
        Some(ThinkingSetting::ProviderDefault)
    } else {
        matches
            .get_one::<String>("thinking")
            .map(|value| ThinkingSetting::Choice {
                value: value.clone(),
            })
    }
}

pub(crate) fn explicit_step_setting(
    setting: Option<&ThinkingSetting<crate::RunArg>>,
    data: &serde_json::Value,
) -> Result<Option<ThinkingSetting>, String> {
    match setting {
        None => Ok(None),
        Some(ThinkingSetting::ProviderDefault) => Ok(Some(ThinkingSetting::ProviderDefault)),
        Some(ThinkingSetting::Choice { value }) => {
            let value = match value {
                crate::RunArg::Literal(value) => value.clone(),
                crate::RunArg::Variable(name) => lookup_action_variable(data, name)
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| format!("Thinking variable '{name}' must resolve to a string."))?
                    .to_owned(),
            };
            if value.trim().is_empty() {
                return Err("Thinking choice must be a nonempty string.".into());
            }
            Ok(Some(ThinkingSetting::Choice { value }))
        }
    }
}

pub(crate) fn step_setting(
    step: &crate::RunStep,
    data: &serde_json::Value,
    selected_profile: Option<&ActionProviderContext>,
    invocation: &ActionProviderContext,
) -> Result<(Option<ThinkingSetting>, String), String> {
    if let Some(setting) = explicit_step_setting(step.thinking.as_ref(), data)? {
        return Ok((Some(setting), "step".into()));
    }
    if let Some(profile) = selected_profile {
        if let Some(setting) = &profile.thinking {
            return Ok((Some(setting.clone()), "step_profile".into()));
        }
    }
    Ok((
        invocation.thinking.clone(),
        invocation.thinking_source.clone(),
    ))
}

pub(crate) async fn resolve_for_request(
    selection: Option<&ThinkingSetting>,
    source: &str,
    context: &ActionProviderContext,
    model: &str,
    kind: ThinkingRequestKind,
    budget: InvocationRuntimeBudget,
) -> Result<ThinkingOutcome, String> {
    let support = if matches!(selection, Some(ThinkingSetting::Choice { .. })) {
        let remaining = remaining_runtime_duration(budget, "before resolving thinking support")?;
        tokio::time::timeout(
            remaining,
            request_support(
                context.provider,
                &context.url,
                model,
                &context.token,
                context.openai_account_id.as_deref(),
                kind,
            ),
        )
        .await
        .map_err(|_| "Runtime budget expired while resolving thinking support.".to_string())?
    } else {
        // A default run must not depend on discovery or a reachable catalog.
        ThinkingSupport::Unknown {
            reason: "metadata was not requested for provider-default execution".into(),
            evidence: None,
        }
    };
    Ok(thinking::resolve(selection, source, support))
}

pub(crate) fn step_scope(
    action_index: usize,
    action_name: &str,
    step_index: usize,
) -> serde_json::Value {
    serde_json::json!({"kind":"action_step","action_index":action_index,"action":action_name,"step_index":step_index.saturating_sub(1)})
}

fn cargo_ai_artifact_on_path() -> Option<std::path::PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|root| {
        let artifact = root.join(format!("cargo-ai{}", std::env::consts::EXE_SUFFIX));
        if !artifact.is_file() {
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if artifact.metadata().ok()?.permissions().mode() & 0o111 == 0 {
                return None;
            }
        }
        Some(artifact)
    })
}

pub(super) fn child_thinking(
    invocation: &ChildArtifactInvocation,
    setting: &ThinkingSetting,
    command: &mut tokio::process::Command,
    scope: serde_json::Value,
) -> serde_json::Value {
    let (artifact, cli_run) = match invocation {
        ChildArtifactInvocation::DirectExecutable(path) => (Some(path.clone()), false),
        #[cfg(cargo_ai_cli)]
        ChildArtifactInvocation::CargoSubcommand(_)
        | ChildArtifactInvocation::StandaloneCargoAi(_) => (cargo_ai_artifact_on_path(), true),
        #[cfg(not(cargo_ai_cli))]
        ChildArtifactInvocation::CargoSubcommand | ChildArtifactInvocation::StandaloneCargoAi => {
            (cargo_ai_artifact_on_path(), true)
        }
    };
    let capable = artifact
        .as_deref()
        .and_then(|path| crate::generated_capabilities::capabilities_for_artifact(path).ok())
        .is_some_and(|capability| {
            capability.supports_thinking() && capability.is_cli_run() == cli_run
        });
    if capable {
        match setting {
            ThinkingSetting::ProviderDefault => {
                command.arg("--thinking-provider-default");
            }
            ThinkingSetting::Choice { value } => {
                command.arg("--thinking").arg(value);
            }
        }
    }
    serde_json::json!({"scope":scope,"kind":"child_forwarding","requested":setting,
        "disposition":if capable {"forwarded"} else {"not_forwarded"},
        "declaration_source":if capable {"embedded_artifact"} else {"unavailable"},
        "effective":"child_unverified"})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_references_require_strings_and_preserve_exact_values() {
        let reference = ThinkingSetting::Choice {
            value: crate::RunArg::Variable("runtime.effort".into()),
        };
        for value in [
            serde_json::json!(true),
            serde_json::json!(3),
            serde_json::json!(null),
            serde_json::json!(""),
        ] {
            assert!(explicit_step_setting(
                Some(&reference),
                &serde_json::json!({"runtime":{"effort":value}})
            )
            .is_err());
        }
        let exact = explicit_step_setting(
            Some(&reference),
            &serde_json::json!({"runtime":{"effort":"Ultra"}}),
        )
        .unwrap();
        assert_eq!(
            exact,
            Some(ThinkingSetting::Choice {
                value: "Ultra".into()
            })
        );
        assert_eq!(
            explicit_step_setting(
                Some(&ThinkingSetting::ProviderDefault),
                &serde_json::json!({})
            )
            .unwrap(),
            Some(ThinkingSetting::ProviderDefault)
        );
    }

    #[test]
    fn thinking_action_choices_preserve_long_literal_and_reference_values() {
        let choice = "exact-choice-".repeat(40);
        let data = serde_json::json!({"choice":choice});
        for value in [
            crate::RunArg::Literal(choice.clone()),
            crate::RunArg::Variable("choice".into()),
        ] {
            let selection = explicit_step_setting(Some(&ThinkingSetting::Choice { value }), &data)
                .unwrap()
                .unwrap();
            let support = ThinkingSupport::Configurable {
                choices: vec![crate::providers::thinking::ThinkingChoice {
                    value: choice.clone(),
                    description: None,
                }],
                default: None,
                evidence: None,
            };
            let applied = thinking::resolve(Some(&selection), "step", support);
            assert_eq!(applied.applied_choice(), Some(choice.as_str()));
            let unavailable = thinking::resolve(
                Some(&selection),
                "step",
                ThinkingSupport::Configurable {
                    choices: vec![crate::providers::thinking::ThinkingChoice {
                        value: "high".into(),
                        description: None,
                    }],
                    default: None,
                    evidence: None,
                },
            );
            assert_eq!(unavailable.effective, ThinkingSetting::ProviderDefault);
            assert_eq!(
                unavailable.fallback,
                Some(crate::providers::thinking::ThinkingFallback::UnavailableChoice)
            );
        }
    }

    #[test]
    fn thinking_scope_indexes_are_zero_based() {
        assert_eq!(
            step_scope(2, "action", 3),
            serde_json::json!({"kind":"action_step","action_index":2,"action":"action","step_index":2})
        );
    }

    #[cfg(cargo_ai_cli)]
    fn context(setting: Option<ThinkingSetting>, source: &str) -> ActionProviderContext {
        ActionProviderContext {
            project_data: None,
            provider: crate::providers::ProviderKind::Ollama,
            profile_name: None,
            auth_mode: "none".into(),
            model: "fixture".into(),
            thinking: setting,
            thinking_source: source.into(),
            max_output_tokens: None,
            url: "http://127.0.0.1:1/v1/chat/completions".into(),
            token: String::new(),
            openai_account_id: None,
            inference_timeout_in_sec: 1,
            tool_resolver: None,
            package_context: None,
            usage_log: None,
        }
    }

    #[cfg(cargo_ai_cli)]
    #[test]
    fn thinking_step_and_profile_precedence_preserves_explicit_default() {
        let mut step: crate::RunStep = serde_json::from_value(serde_json::json!({
            "kind":"generate_image", "args":[], "tool_params":{}, "ignore_tools":false
        }))
        .unwrap();
        let invocation = context(
            Some(ThinkingSetting::Choice {
                value: "high".into(),
            }),
            "invocation",
        );
        let mut profile = context(
            Some(ThinkingSetting::Choice {
                value: "low".into(),
            }),
            "profile",
        );
        let data = serde_json::json!({"requested":"max"});
        assert_eq!(
            step_setting(&step, &data, None, &invocation).unwrap(),
            (invocation.thinking.clone(), "invocation".into())
        );
        assert_eq!(
            step_setting(&step, &data, Some(&profile), &invocation).unwrap(),
            (profile.thinking.clone(), "step_profile".into())
        );
        profile.thinking = Some(ThinkingSetting::ProviderDefault);
        assert_eq!(
            step_setting(&step, &data, Some(&profile), &invocation).unwrap(),
            (
                Some(ThinkingSetting::ProviderDefault),
                "step_profile".into()
            )
        );
        step.thinking = Some(ThinkingSetting::Choice {
            value: crate::RunArg::Variable("requested".into()),
        });
        assert_eq!(
            step_setting(&step, &data, Some(&profile), &invocation).unwrap(),
            (
                Some(ThinkingSetting::Choice {
                    value: "max".into()
                }),
                "step".into()
            )
        );
        step.thinking = Some(ThinkingSetting::ProviderDefault);
        assert_eq!(
            step_setting(&step, &data, Some(&profile), &invocation).unwrap(),
            (Some(ThinkingSetting::ProviderDefault), "step".into())
        );
        step.thinking = None;
        profile.thinking = None;
        assert_eq!(
            step_setting(&step, &data, Some(&profile), &invocation).unwrap(),
            (invocation.thinking.clone(), "invocation".into())
        );
    }

    #[cfg(cargo_ai_cli)]
    #[tokio::test]
    async fn thinking_default_execution_never_needs_metadata_io() {
        let context = context(None, "provider_default");
        let budget = InvocationRuntimeBudget {
            max_runtime_secs: 1,
            started_at_ms: 0,
            deadline_ms: 0,
        };
        for setting in [None, Some(ThinkingSetting::ProviderDefault)] {
            let outcome = resolve_for_request(
                setting.as_ref(),
                "invocation",
                &context,
                "fixture",
                ThinkingRequestKind::Text,
                budget,
            )
            .await
            .unwrap();
            assert_eq!(outcome.effective, ThinkingSetting::ProviderDefault);
            assert!(outcome.fallback.is_none());
        }
        assert!(resolve_for_request(
            Some(&ThinkingSetting::Choice {
                value: "high".into()
            }),
            "invocation",
            &context,
            "fixture",
            ThinkingRequestKind::Text,
            budget
        )
        .await
        .is_err());
    }
}
