//! Immutable grants for supported declarative runtime selections.
use crate::providers::thinking::ThinkingSetting;
use serde::{Deserialize, Serialize};
use std::future::Future;

pub const CHILD_POLICY_ENV: &str = "CARGO_AI_EXECUTION_POLICY_V1";
const CHILD_POLICY_REQUIRED_ENV: &str = "CARGO_AI_EXECUTION_POLICY_REQUIRED_V1";
pub const MAX_POLICY_BYTES: usize = 32 * 1024;
const MAX_COMBINATIONS: usize = 128;

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BoundaryCounts {
    pub credentials: usize,
    pub discovery: usize,
    pub child_spawn: usize,
}
#[cfg(test)]
thread_local! { static BOUNDARY_COUNTS: std::cell::Cell<BoundaryCounts> = const { std::cell::Cell::new(BoundaryCounts { credentials:0, discovery:0, child_spawn:0 }) }; }
#[cfg(test)]
pub(crate) fn boundary_counts() -> BoundaryCounts {
    BOUNDARY_COUNTS.with(std::cell::Cell::get)
}
#[cfg(test)]
pub(crate) fn note_boundary(kind: &str) {
    BOUNDARY_COUNTS.with(|counts| {
        let mut value = counts.get();
        match kind {
            "credentials" => value.credentials += 1,
            "discovery" => value.discovery += 1,
            "child_spawn" => value.child_spawn += 1,
            _ => unreachable!(),
        }
        counts.set(value);
    });
}

tokio::task_local! { static INVOCATION_POLICY: Option<ExecutionPolicy>; }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelSelection {
    Named { value: String },
    ProviderDefault {},
    FixedService {},
}
impl ModelSelection {
    pub fn named_or_default(value: &str) -> Self {
        if value.is_empty() {
            Self::ProviderDefault {}
        } else {
            Self::Named {
                value: value.into(),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    Text,
    Image,
    Audio,
    Transcription,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllowedSelection {
    pub profile: Option<String>,
    pub request_kind: RequestKind,
    pub model: ModelSelection,
    pub thinking: ThinkingSetting,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionLimits {
    pub max_runtime_secs: u64,
    pub max_output_tokens: u32,
    pub max_agent_depth: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPolicy {
    pub version: u32,
    pub allowed: Vec<AllowedSelection>,
    pub limits: ExecutionLimits,
}

fn invalid() -> String {
    failure(
        "action.execution_policy_invalid",
        "Execution policy is malformed, oversized, or unsupported.",
    )
}
fn failure(code: &'static str, message: &'static str) -> String {
    #[cfg(cargo_ai_cli)]
    crate::commands::machine::record_error(crate::commands::machine::Failure::new(code, message));
    #[cfg(not(cargo_ai_cli))]
    let _ = code;
    message.into()
}

#[cfg_attr(not(cargo_ai_cli), allow(dead_code))]
impl ExecutionPolicy {
    pub fn parse(value: &serde_json::Value) -> Result<Self, String> {
        let encoded = serde_json::to_vec(value).map_err(|_| invalid())?;
        Self::parse_bytes(&encoded)
    }
    pub fn parse_bytes(encoded: &[u8]) -> Result<Self, String> {
        if encoded.len() > MAX_POLICY_BYTES {
            return Err(invalid());
        }
        let value: serde_json::Value = serde_json::from_slice(encoded).map_err(|_| invalid())?;
        if value
            .get("allowed")
            .and_then(serde_json::Value::as_array)
            .is_none_or(|grants| grants.iter().any(|grant| grant.get("profile").is_none()))
        {
            return Err(invalid());
        }
        let policy: Self = serde_json::from_value(value).map_err(|_| invalid())?;
        if policy.version != 1
            || policy.allowed.len() > MAX_COMBINATIONS
            || policy.limits.max_runtime_secs == 0
            || policy.limits.max_runtime_secs > 86_400
            || policy.limits.max_output_tokens == 0
            || policy.limits.max_agent_depth > 64
        {
            return Err(invalid());
        }
        for grant in &policy.allowed {
            if grant
                .profile
                .as_ref()
                .is_some_and(|v| v.trim().is_empty() || v.len() > 256)
                || matches!(&grant.model, ModelSelection::Named {value} if value.trim().is_empty() || value.len() > 1024)
                || matches!(&grant.thinking, ThinkingSetting::Choice {value} if value.trim().is_empty() || value.len() > 1024)
            {
                return Err(invalid());
            }
        }
        Ok(policy)
    }
    pub async fn scope<F: Future>(&self, future: F) -> F::Output {
        INVOCATION_POLICY.scope(Some(self.clone()), future).await
    }
    fn allows(
        &self,
        profile: Option<&str>,
        kind: RequestKind,
        model: &ModelSelection,
        thinking: &ThinkingSetting,
    ) -> bool {
        self.allowed.iter().any(|grant| {
            grant.profile.as_deref() == profile
                && grant.request_kind == kind
                && &grant.model == model
                && &grant.thinking == thinking
        })
    }
}

pub fn current() -> Option<ExecutionPolicy> {
    INVOCATION_POLICY.try_with(Clone::clone).ok().flatten()
}

#[cfg(cargo_ai_cli)]
pub fn inheritance_requested() -> bool {
    std::env::var_os(CHILD_POLICY_ENV).is_some()
        || std::env::var_os(CHILD_POLICY_REQUIRED_ENV).is_some()
}

pub fn inherited() -> Result<Option<ExecutionPolicy>, String> {
    let required = std::env::var_os(CHILD_POLICY_REQUIRED_ENV);
    parse_inherited(required.as_deref(), std::env::var(CHILD_POLICY_ENV))
}

fn parse_inherited(
    required: Option<&std::ffi::OsStr>,
    encoded: Result<String, std::env::VarError>,
) -> Result<Option<ExecutionPolicy>, String> {
    if required.is_some_and(|value| value != "1") {
        return Err(invalid());
    }
    match encoded {
        Ok(encoded) => ExecutionPolicy::parse_bytes(encoded.as_bytes()).map(Some),
        Err(std::env::VarError::NotPresent) if required.is_none() => Ok(None),
        Err(_) => Err(invalid()),
    }
}

/// Establishes an inherited grant before runtime configuration or authentication.
pub async fn scope_inherited<F: Future>(future: F) -> Result<F::Output, String> {
    let inherited = inherited()?;
    let policy = match (current(), inherited) {
        (Some(current), Some(inherited)) => {
            // An outer declared action may narrow, but cannot discard inherited authority.
            if current.limits.max_runtime_secs > inherited.limits.max_runtime_secs
                || current.limits.max_output_tokens > inherited.limits.max_output_tokens
                || current.limits.max_agent_depth > inherited.limits.max_agent_depth
                || current
                    .allowed
                    .iter()
                    .any(|grant| !inherited.allowed.contains(grant))
            {
                return Err(invalid());
            }
            Some(current)
        }
        (current, inherited) => current.or(inherited),
    };
    Ok(INVOCATION_POLICY.scope(policy, future).await)
}

pub async fn scope_optional<F: Future>(policy: Option<ExecutionPolicy>, future: F) -> F::Output {
    INVOCATION_POLICY.scope(policy, future).await
}

pub fn check(
    profile: Option<&str>,
    kind: RequestKind,
    model: ModelSelection,
    thinking: Option<&ThinkingSetting>,
    max_output_tokens: Option<u32>,
) -> Result<(), String> {
    let Some(policy) = current() else {
        return Ok(());
    };
    let thinking = thinking
        .cloned()
        .unwrap_or(ThinkingSetting::ProviderDefault);
    if !policy.allows(profile, kind, &model, &thinking) {
        return Err(failure(
            "action.execution_selection_denied",
            "Effective runtime selection is outside the execution policy.",
        ));
    }
    if max_output_tokens.is_some_and(|value| value > policy.limits.max_output_tokens) {
        return Err(failure(
            "action.execution_limit_denied",
            "Effective runtime limit exceeds the execution policy.",
        ));
    }
    Ok(())
}

pub fn check_child_selectors(
    profile: Option<&str>,
    thinking: Option<&ThinkingSetting>,
) -> Result<(), String> {
    let Some(policy) = current() else {
        return Ok(());
    };
    if !policy.allowed.iter().any(|grant| {
        grant.request_kind == RequestKind::Text
            && profile.is_none_or(|profile| grant.profile.as_deref() == Some(profile))
            && thinking.is_none_or(|thinking| &grant.thinking == thinking)
    }) {
        return Err(failure(
            "action.execution_selection_denied",
            "Effective child selection is outside the execution policy.",
        ));
    }
    Ok(())
}

/// Explicit caller requests cannot expand ceilings; omitted/default values are capped.
pub fn check_explicit_limits(
    output_tokens: Option<u32>,
    runtime_secs: Option<u64>,
    agent_depth: Option<u32>,
) -> Result<(), String> {
    let Some(policy) = current() else {
        return Ok(());
    };
    if output_tokens.is_some_and(|value| value > policy.limits.max_output_tokens)
        || runtime_secs.is_some_and(|value| value > policy.limits.max_runtime_secs)
        || agent_depth.is_some_and(|value| value > policy.limits.max_agent_depth)
    {
        return Err(failure(
            "action.execution_limit_denied",
            "Explicit runtime limit exceeds the execution policy.",
        ));
    }
    Ok(())
}

fn executable_candidates(
    program: &std::path::Path,
    windows: bool,
    pathext: &str,
) -> Vec<std::path::PathBuf> {
    if !windows || program.extension().is_some() {
        return vec![program.to_path_buf()];
    }
    let mut extensions = pathext
        .split(';')
        .filter_map(|extension| {
            let extension = extension.trim().to_ascii_lowercase();
            (extension.starts_with('.')
                && extension.len() <= 16
                && extension[1..]
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric())
                && extension.len() > 1)
                .then_some(extension)
        })
        .take(16)
        .collect::<Vec<_>>();
    if !extensions.iter().any(|extension| extension == ".exe") {
        extensions.push(".exe".into());
    }
    extensions
        .into_iter()
        .map(|extension| {
            let mut name = program.as_os_str().to_os_string();
            name.push(extension);
            std::path::PathBuf::from(name)
        })
        .collect()
}

fn executable_on_path(
    program: &std::path::Path,
    roots: impl IntoIterator<Item = std::path::PathBuf>,
    windows: bool,
    pathext: &str,
) -> Option<std::path::PathBuf> {
    let candidates = executable_candidates(program, windows, pathext);
    roots.into_iter().find_map(|root| {
        candidates.iter().find_map(|name| {
            let path = root.join(name);
            let metadata = path.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if !windows && metadata.permissions().mode() & 0o111 == 0 {
                    return None;
                }
            }
            std::fs::canonicalize(path).ok()
        })
    })
}

pub fn output_limit(selected: Option<u32>) -> Option<u32> {
    current()
        .map(|policy| {
            selected
                .unwrap_or(policy.limits.max_output_tokens)
                .min(policy.limits.max_output_tokens)
        })
        .or(selected)
}
pub fn runtime_limit(selected: u64) -> u64 {
    current()
        .map(|policy| selected.min(policy.limits.max_runtime_secs))
        .unwrap_or(selected)
}

pub fn depth_limit(selected: u32) -> u32 {
    current()
        .map(|policy| selected.min(policy.limits.max_agent_depth))
        .unwrap_or(selected)
}

/// Checks and binds the exact executable launched, independently of thinking telemetry.
pub fn propagate_child(command: &mut tokio::process::Command, cli_run: bool) -> Result<(), String> {
    let Some(policy) = current() else {
        return Ok(());
    };
    let selected = std::path::PathBuf::from(command.as_std().get_program());
    let artifact = if selected.components().count() > 1 {
        std::fs::canonicalize(&selected).ok()
    } else {
        std::env::var_os("PATH").and_then(|paths| {
            executable_on_path(
                &selected,
                std::env::split_paths(&paths),
                cfg!(windows),
                &std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into()),
            )
        })
    }
    .ok_or_else(|| {
        failure(
            "action.execution_policy_unsupported",
            "Child runtime cannot enforce execution policy; rebuild or select a supported runtime.",
        )
    })?;
    let capabilities = crate::generated_capabilities::capabilities_for_artifact(&artifact).map_err(|_| failure("action.execution_policy_unsupported", "Child runtime cannot enforce execution policy; rebuild or select a supported runtime."))?;
    if capabilities.is_cli_run() != cli_run || !capabilities.supports_execution_policy() {
        return Err(failure(
            "action.execution_policy_unsupported",
            "Child runtime cannot enforce execution policy; rebuild or select a supported runtime.",
        ));
    }
    // Tokio does not expose a program setter. Rebuild while retaining args, cwd and env.
    let old = command.as_std();
    let mut bound = tokio::process::Command::new(&artifact);
    bound.args(old.get_args());
    if let Some(directory) = old.get_current_dir() {
        bound.current_dir(directory);
    }
    for (key, value) in old.get_envs() {
        match value {
            Some(value) => {
                bound.env(key, value);
            }
            None => {
                bound.env_remove(key);
            }
        }
    }
    let encoded = serde_json::to_string(&policy).map_err(|_| invalid())?;
    if encoded.len() > MAX_POLICY_BYTES {
        return Err(invalid());
    }
    bound.env(CHILD_POLICY_ENV, encoded);
    bound.env(CHILD_POLICY_REQUIRED_ENV, "1");
    *command = bound;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy() -> ExecutionPolicy {
        ExecutionPolicy::parse(&serde_json::json!({"version":1,"allowed":[{"profile":null,"request_kind":"text","model":{"kind":"named","value":"fixture"},"thinking":{"mode":"provider_default"}}],"limits":{"max_runtime_secs":60,"max_output_tokens":100,"max_agent_depth":8}})).unwrap()
    }
    #[test]
    fn policy_is_bounded_and_explicit() {
        let policy = policy();
        let mut value = serde_json::to_value(policy).unwrap();
        value["allowed"][0]
            .as_object_mut()
            .unwrap()
            .remove("thinking");
        assert!(ExecutionPolicy::parse(&value).is_err());
        assert!(ExecutionPolicy::parse_bytes(&vec![b' '; MAX_POLICY_BYTES + 1]).is_err());
        assert!(ExecutionPolicy::parse(&serde_json::json!(null)).is_err());
    }
    #[tokio::test]
    async fn explicit_combinations_do_not_form_a_cartesian_product() {
        let mut policy = policy();
        let mut grant = policy.allowed[0].clone();
        grant.profile = Some("other".into());
        grant.model = ModelSelection::Named {
            value: "other-model".into(),
        };
        policy.allowed.push(grant);
        policy
            .scope(async {
                assert!(check(
                    None,
                    RequestKind::Text,
                    ModelSelection::named_or_default("fixture"),
                    None,
                    None
                )
                .is_ok());
                assert!(check(
                    Some("other"),
                    RequestKind::Text,
                    ModelSelection::named_or_default("fixture"),
                    None,
                    None
                )
                .is_err());
                assert!(check(
                    None,
                    RequestKind::Image,
                    ModelSelection::named_or_default("fixture"),
                    None,
                    None
                )
                .is_err());
                assert!(check(
                    None,
                    RequestKind::Text,
                    ModelSelection::ProviderDefault {},
                    None,
                    None
                )
                .is_err());
                assert_eq!(output_limit(None), Some(100));
                assert_eq!(runtime_limit(500), 60);
            })
            .await;
        assert!(current().is_none());
        assert!(check(
            None,
            RequestKind::Image,
            ModelSelection::ProviderDefault {},
            None,
            None
        )
        .is_ok());
    }
    #[tokio::test]
    async fn parallel_lanes_keep_independent_immutable_grants() {
        let policy = policy();
        policy
            .scope(async {
                let handles = (0..2)
                    .map(|_| {
                        let inherited = current();
                        tokio::spawn(async move {
                            scope_optional(inherited, async {
                                assert!(check(
                                    None,
                                    RequestKind::Text,
                                    ModelSelection::named_or_default("fixture"),
                                    None,
                                    None
                                )
                                .is_ok());
                                assert!(check(
                                    None,
                                    RequestKind::Text,
                                    ModelSelection::named_or_default("ungranted"),
                                    None,
                                    None
                                )
                                .is_err());
                            })
                            .await
                        })
                    })
                    .collect::<Vec<_>>();
                for handle in handles {
                    handle.await.unwrap();
                }
            })
            .await;
    }
    #[tokio::test]
    async fn child_propagation_requires_passive_enforcement_and_binds_exact_file() {
        let root = std::env::temp_dir().join(format!("cargo-ai-policy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let child = root.join("child");
        policy()
            .scope(async {
                for bytes in [
                    b"opaque".to_vec(),
                    crate::generated_capabilities::test_record(2),
                ] {
                    std::fs::write(&child, bytes).unwrap();
                    let mut command = tokio::process::Command::new(&child);
                    assert!(propagate_child(&mut command, false).is_err());
                }
                std::fs::write(&child, crate::generated_capabilities::test_record(3)).unwrap();
                let mut command = tokio::process::Command::new(&child);
                command.arg("business-input").env("FIXTURE_ENV", "retained");
                propagate_child(&mut command, false).unwrap();
                assert_eq!(
                    std::path::PathBuf::from(command.as_std().get_program()),
                    std::fs::canonicalize(&child).unwrap()
                );
                let encoded = command
                    .as_std()
                    .get_envs()
                    .find(|(key, _)| key == &std::ffi::OsStr::new(CHILD_POLICY_ENV))
                    .unwrap()
                    .1
                    .unwrap()
                    .to_str()
                    .unwrap();
                assert_eq!(
                    ExecutionPolicy::parse_bytes(encoded.as_bytes()).unwrap(),
                    policy()
                );
                assert_eq!(
                    command.as_std().get_args().collect::<Vec<_>>(),
                    vec![std::ffi::OsStr::new("business-input")]
                );
                assert_eq!(boundary_counts().child_spawn, 0);
            })
            .await;
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn malformed_present_inheritance_never_becomes_unrestricted() {
        let required = Some(std::ffi::OsStr::new("1"));
        assert_eq!(
            parse_inherited(None, Err(std::env::VarError::NotPresent)).unwrap(),
            None
        );
        assert!(parse_inherited(required, Err(std::env::VarError::NotPresent)).is_err());
        for encoded in [
            String::new(),
            "null".into(),
            "{}".into(),
            "x".repeat(MAX_POLICY_BYTES + 1),
        ] {
            for marker in [None, required] {
                assert!(parse_inherited(marker, Ok(encoded.clone())).is_err());
            }
        }
        for marker in ["", "0", "true"] {
            assert!(parse_inherited(
                Some(std::ffi::OsStr::new(marker)),
                Ok(serde_json::to_string(&policy()).unwrap())
            )
            .is_err());
        }
        for marker in [None, required] {
            assert!(parse_inherited(
                marker,
                Err(std::env::VarError::NotUnicode(std::ffi::OsString::from(
                    "non-Unicode fixture"
                )))
            )
            .is_err());
            assert_eq!(
                parse_inherited(marker, Ok(serde_json::to_string(&policy()).unwrap())).unwrap(),
                Some(policy())
            );
        }
    }
    #[test]
    fn windows_path_lookup_binds_exe_suffix_and_respects_pathext_order() {
        let root =
            std::env::temp_dir().join(format!("cargo-ai-policy-path-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        for name in ["cargo-ai.exe", "cargo-ai.com"] {
            std::fs::write(root.join(name), b"fixture").unwrap();
        }
        let resolve = |name, extensions| {
            executable_on_path(
                std::path::Path::new(name),
                vec![root.clone()],
                true,
                extensions,
            )
            .unwrap()
        };
        assert_eq!(
            resolve("cargo-ai", ".EXE;.COM"),
            std::fs::canonicalize(root.join("cargo-ai.exe")).unwrap()
        );
        assert_eq!(
            resolve("cargo-ai", ".COM;.EXE"),
            std::fs::canonicalize(root.join("cargo-ai.com")).unwrap()
        );
        assert_eq!(
            resolve("cargo-ai.exe", ".COM"),
            std::fs::canonicalize(root.join("cargo-ai.exe")).unwrap()
        );
        assert_eq!(
            resolve("cargo-ai", ""),
            std::fs::canonicalize(root.join("cargo-ai.exe")).unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn explicit_limit_expansion_denied_while_defaults_are_capped() {
        let policy = policy();
        policy
            .scope(async {
                for limits in [
                    (Some(101), None, None),
                    (None, Some(61), None),
                    (None, None, Some(9)),
                ] {
                    assert!(check_explicit_limits(limits.0, limits.1, limits.2).is_err());
                }
                assert!(check_explicit_limits(Some(100), Some(60), Some(8)).is_ok());
                assert!(check_explicit_limits(None, None, None).is_ok());
                assert_eq!(output_limit(Some(500)), Some(100));
                assert_eq!(runtime_limit(600), 60);
                assert_eq!(depth_limit(20), 8);
            })
            .await;
        assert!(check_explicit_limits(Some(500), Some(600), Some(20)).is_ok());
    }
}
