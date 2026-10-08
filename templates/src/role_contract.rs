//! Portable role declarations and pure, scoped resolution. Bindings never confer authority.
#[cfg(any(cargo_ai_cli, test))]
use crate::execution_policy::RequestKind;
use crate::execution_policy::{AllowedSelection, ExecutionLimits, ModelSelection};
#[cfg(any(cargo_ai_cli, test))]
use crate::providers::thinking::ThinkingSetting;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
#[cfg(any(cargo_ai_cli, test))]
use std::collections::BTreeSet;

#[cfg(any(cargo_ai_cli, test))]
pub const ROLE_CONTRACT_VERSION: u32 = 1;
#[cfg(any(cargo_ai_cli, test))]
pub const MAX_ROLES: usize = 64;
#[cfg(any(cargo_ai_cli, test))]
pub const MAX_CALL_SITES: usize = 256;
#[cfg(any(cargo_ai_cli, test))]
pub const MAX_CONTEXTS: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OperationRequirements {
    pub operation: String,
    #[serde(default)]
    pub input_modalities: Vec<String>,
    #[serde(default)]
    pub structured_output: bool,
    /// Each setting has a finite allowed set. No cross-operation settings inheritance.
    #[serde(default)]
    pub settings: BTreeMap<String, Vec<Value>>,
}

#[cfg(any(cargo_ai_cli, test))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RoleDeclaration {
    pub id: String,
    pub label: String,
    pub purpose: String,
    pub requirements: OperationRequirements,
    #[serde(default)]
    pub different_model_from: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recommendations: Vec<RoleRecommendation>,
}

/// Portable suggestions are descriptive data, never bindings or capability evidence.
#[cfg(any(cargo_ai_cli, test))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RoleRecommendation {
    pub provider: String,
    pub model: String,
    pub operation: String,
    #[serde(default)]
    pub settings: BTreeMap<String, Value>,
    pub provenance: Vec<String>,
    pub rationale: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct CallLocator {
    pub definition: String,
    /// `root`, `actions.<index>.run.<index>` or a declared `tools.<name>.<site>`.
    pub site: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CallKind {
    Root,
    Image,
    Audio,
    Transcription,
    Child,
    ToolChild,
}
#[cfg(any(cargo_ai_cli, test))]
impl CallKind {
    pub fn request_kind(self) -> RequestKind {
        match self {
            Self::Root | Self::Child | Self::ToolChild => RequestKind::Text,
            Self::Image => RequestKind::Image,
            Self::Audio => RequestKind::Audio,
            Self::Transcription => RequestKind::Transcription,
        }
    }
    pub fn operation(self) -> &'static str {
        match self {
            Self::Root | Self::Child | Self::ToolChild => "text_generation",
            Self::Image => "image_generation",
            Self::Audio => "speech_generation",
            Self::Transcription => "transcription",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FixedSelection {
    pub profile: Option<String>,
    pub model: ModelSelection,
    #[serde(default)]
    pub settings: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CallSite {
    pub id: String,
    pub locator: CallLocator,
    pub kind: CallKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<FixedSelection>,
    /// Additional per-use constraints must also hold for the shared binding.
    pub requirements: OperationRequirements,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Exact generated executable referenced by a direct child step, when distinct from JSON target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
    /// A tool child accepts only this bounded business schema, never selectors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct ContextKey {
    pub action: String,
    pub interface: String,
    pub mode: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RoleContext {
    pub key: ContextKey,
    pub call_sites: Vec<String>,
    #[serde(default)]
    pub resources: Vec<String>,
    #[serde(default)]
    pub data_scopes: Vec<String>,
    pub limits: ExecutionLimits,
}

#[cfg(any(cargo_ai_cli, test))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RoleRegistry {
    pub version: u32,
    pub roles: Vec<RoleDeclaration>,
    pub call_sites: Vec<CallSite>,
    pub contexts: Vec<RoleContext>,
    /// Exact portable source files for tools used by the declared native call graph.
    #[serde(default)]
    pub tool_content: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RoleBinding {
    pub role: String,
    pub profile: String,
    pub profile_uuid: String,
    pub connection_generation: String,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub settings: BTreeMap<String, Value>,
}
impl std::fmt::Debug for RoleBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RoleBinding { private: [redacted] }")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BindingRevision {
    pub version: u32,
    pub revision: String,
    pub bindings: Vec<RoleBinding>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResolutionContext {
    pub project_identity: String,
    pub environment_identity: String,
    /// Identity of the complete verified catalog, content and installation provenance.
    pub package_identity: String,
    pub runtime_contract: String,
    /// Native protected context evidence, not a host assertion of unchanged credentials.
    pub connection_context_identity: String,
    /// Host project ceiling and consent identities remain scoped to this action context.
    pub consent_identity: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Compatibility {
    Compatible,
    Incompatible,
    Unknown,
    Stale,
    Unavailable,
}

/// Supplied by native capability discovery after validating the selected private context.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CapabilityEvidence {
    pub profile_uuid: String,
    pub connection_generation: String,
    pub provider: String,
    pub model: String,
    pub operation: String,
    pub input_modalities: Vec<String>,
    pub structured_output: bool,
    pub settings: BTreeMap<String, Value>,
    pub status: Compatibility,
    pub evidence_revision: String,
    pub provenance: Vec<String>,
    pub valid_until_unix_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_evidence: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResolvedCall {
    pub call_site: CallSite,
    pub selection: Option<AllowedSelection>,
    pub settings: BTreeMap<String, Value>,
    pub compatibility: Compatibility,
    pub reason: String,
    pub evidence: Vec<CapabilityEvidence>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Resolution {
    pub version: u32,
    pub identity: String,
    pub binding_revision: String,
    pub binding_identity: String,
    pub context: ResolutionContext,
    pub scope: RoleContext,
    pub calls: Vec<ResolvedCall>,
    pub required_selections: Vec<AllowedSelection>,
    pub ready: bool,
    pub execution_authorized: bool,
    pub invocation_access: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleError {
    pub code: &'static str,
    pub message: &'static str,
}
impl std::fmt::Display for RoleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
fn invalid() -> RoleError {
    RoleError {
        code: "role.invalid_declaration",
        message: "The portable role declaration is invalid or unsupported.",
    }
}
#[cfg(any(cargo_ai_cli, test))]
fn binding_error() -> RoleError {
    RoleError {
        code: "role.invalid_bindings",
        message: "The private role binding revision is invalid or unsupported.",
    }
}
pub fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}
#[cfg(any(cargo_ai_cli, test))]
pub fn portable_definition(value: &str) -> bool {
    value.ends_with(".json")
        && value.len() <= 1024
        && !value.contains(['\\', ':'])
        && value
            .split('/')
            .all(|part| !part.is_empty() && !matches!(part, "." | ".."))
}
#[cfg(any(cargo_ai_cli, test))]
fn distinct(values: &[String], limit: usize) -> bool {
    values.len() <= limit
        && values.iter().all(|s| identifier(s))
        && values.iter().collect::<BTreeSet<_>>().len() == values.len()
}
#[cfg(any(cargo_ai_cli, test))]
fn settings_valid(settings: &BTreeMap<String, Value>) -> bool {
    settings.len() <= 16
        && settings.iter().all(|(key, value)| {
            matches!(
                key.as_str(),
                "thinking" | "voice" | "format" | "temperature"
            ) && serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() <= 1024)
                && match key.as_str() {
                    "thinking" => serde_json::from_value::<ThinkingSetting>(value.clone()).is_ok(),
                    "temperature" => value
                        .as_f64()
                        .is_some_and(|n| n.is_finite() && (0.0..=2.0).contains(&n)),
                    _ => value
                        .as_str()
                        .is_some_and(|s| !s.is_empty() && s.len() <= 128),
                }
        })
}
#[cfg(any(cargo_ai_cli, test))]
fn validate_requirements(requirements: &OperationRequirements) -> Result<(), RoleError> {
    if ![
        "text_generation",
        "image_generation",
        "speech_generation",
        "transcription",
    ]
    .contains(&requirements.operation.as_str())
        || !distinct(&requirements.input_modalities, 4)
        || requirements
            .input_modalities
            .iter()
            .any(|m| !["text", "image", "audio"].contains(&m.as_str()))
        || requirements.settings.len() > 16
    {
        return Err(invalid());
    }
    for (key, values) in &requirements.settings {
        if values.is_empty()
            || values.len() > 32
            || values
                .iter()
                .any(|value| !settings_valid(&BTreeMap::from([(key.clone(), value.clone())])))
        {
            return Err(invalid());
        }
    }
    Ok(())
}
#[cfg(any(cargo_ai_cli, test))]
pub fn validate_registry(registry: &RoleRegistry) -> Result<(), RoleError> {
    if registry.version != ROLE_CONTRACT_VERSION {
        return Err(RoleError {
            code: "role.unsupported_contract",
            message: "This role declaration requires a supported native role contract.",
        });
    }
    if registry.roles.len() > MAX_ROLES
        || registry.call_sites.len() > MAX_CALL_SITES
        || registry.contexts.is_empty()
        || registry.contexts.len() > MAX_CONTEXTS
        || registry.tool_content.len() > 64
    {
        return Err(invalid());
    }
    let mut roles = BTreeSet::new();
    for role in &registry.roles {
        if !identifier(&role.id)
            || !roles.insert(&role.id)
            || role.label.is_empty()
            || role.label.len() > 4096
            || role.purpose.is_empty()
            || role.purpose.len() > 4096
            || !distinct(&role.different_model_from, MAX_ROLES)
            || role.recommendations.len() > 8
        {
            return Err(invalid());
        }
        validate_requirements(&role.requirements)?;
        for recommendation in &role.recommendations {
            let text = |value: &str, limit| {
                !value.trim().is_empty()
                    && value.len() <= limit
                    && !value.chars().any(char::is_control)
            };
            if !text(&recommendation.provider, 256)
                || !text(&recommendation.model, 256)
                || !matches!(
                    recommendation.operation.as_str(),
                    "text_generation" | "image_generation" | "speech_generation" | "transcription"
                )
                || !settings_valid(&recommendation.settings)
                || recommendation.provenance.is_empty()
                || recommendation.provenance.len() > 8
                || recommendation
                    .provenance
                    .iter()
                    .any(|source| !text(source, 2048))
                || !text(&recommendation.rationale, 2048)
            {
                return Err(invalid());
            }
        }
    }
    for role in &registry.roles {
        if role
            .different_model_from
            .iter()
            .any(|id| id == &role.id || !roles.contains(id))
        {
            return Err(invalid());
        }
    }
    let mut sites = BTreeSet::new();
    let mut locators = BTreeSet::new();
    for site in &registry.call_sites {
        if !identifier(&site.id)
            || !sites.insert(&site.id)
            || !locators.insert(&site.locator)
            || !portable_definition(&site.locator.definition)
            || !identifier(&site.locator.site)
            || (site.role.is_some() && site.fixed.is_some())
            || (site.role.is_none()
                && site.fixed.is_none()
                && !matches!(site.kind, CallKind::Child | CallKind::ToolChild))
            || site.role.as_ref().is_some_and(|id| !roles.contains(id))
            || site.requirements.operation != site.kind.operation()
        {
            return Err(invalid());
        }
        match site.kind {
            CallKind::Root if site.locator.site != "root" || site.target.is_some() => {
                return Err(invalid())
            }
            CallKind::Child | CallKind::ToolChild
                if site
                    .target
                    .as_deref()
                    .is_none_or(|s| !portable_definition(s)) =>
            {
                return Err(invalid())
            }
            CallKind::Image | CallKind::Audio | CallKind::Transcription
                if site.target.is_some() =>
            {
                return Err(invalid())
            }
            _ => {}
        }
        if let Some(artifact) = &site.artifact {
            if !matches!(site.kind, CallKind::Child | CallKind::ToolChild)
                || artifact.is_empty()
                || artifact.len() > 1024
                || artifact.contains(['\\', ':'])
                || artifact
                    .split('/')
                    .any(|p| p.is_empty() || matches!(p, "." | ".."))
            {
                return Err(invalid());
            }
        }
        if site.kind == CallKind::ToolChild {
            if !site.locator.site.starts_with("tools.")
                || site.input_schema.as_ref().is_none_or(|schema| {
                    schema.get("type").and_then(Value::as_str) != Some("object")
                        || schema.get("additionalProperties") != Some(&Value::Bool(false))
                })
            {
                return Err(invalid());
            }
        } else if site.input_schema.is_some() {
            return Err(invalid());
        }
        validate_requirements(&site.requirements)?;
        if let Some(role) = &site.role {
            if registry
                .roles
                .iter()
                .find(|r| &r.id == role)
                .unwrap()
                .requirements
                .operation
                != site.requirements.operation
            {
                return Err(invalid());
            }
        }
        if let Some(fixed) = &site.fixed {
            if !settings_valid(&fixed.settings)
                || fixed
                    .profile
                    .as_ref()
                    .is_some_and(|p| p.is_empty() || p.len() > 256)
                || matches!(&fixed.model, ModelSelection::Named {value} if value.is_empty() || value.len() > 1024)
            {
                return Err(invalid());
            }
        }
    }
    let mut contexts = BTreeSet::new();
    for context in &registry.contexts {
        if !identifier(&context.key.action)
            || !identifier(&context.key.interface)
            || !identifier(&context.key.mode)
            || !contexts.insert(&context.key)
            || !distinct(&context.call_sites, MAX_CALL_SITES)
            || context.call_sites.iter().any(|s| !sites.contains(s))
            || !distinct(&context.resources, 64)
            || !distinct(&context.data_scopes, 64)
            || context.limits.max_runtime_secs == 0
            || context.limits.max_runtime_secs > 86400
            || context.limits.max_output_tokens == 0
            || context.limits.max_agent_depth > 64
        {
            return Err(invalid());
        }
    }
    if sites
        .iter()
        .any(|s| !registry.contexts.iter().any(|c| c.call_sites.contains(s)))
        || roles.iter().any(|r| {
            !registry
                .call_sites
                .iter()
                .any(|s| s.role.as_ref() == Some(*r))
        })
    {
        return Err(invalid());
    }
    for site in &registry.call_sites {
        if let Some(target) = &site.target {
            let target_root = registry.call_sites.iter().find(|candidate| {
                candidate.locator.definition == *target && candidate.kind == CallKind::Root
            });
            let Some(target_root) = target_root else {
                if site.role.is_some() || site.fixed.is_some() {
                    return Err(invalid());
                }
                continue;
            };
            if site.role != target_root.role || site.fixed != target_root.fixed {
                return Err(invalid());
            }
            for context in registry
                .contexts
                .iter()
                .filter(|context| context.call_sites.contains(&site.id))
            {
                if !context.call_sites.contains(&target_root.id) {
                    return Err(invalid());
                }
            }
        }
    }
    Ok(())
}

#[cfg(any(cargo_ai_cli, test))]
pub fn validate_bindings(
    registry: &RoleRegistry,
    revision: &BindingRevision,
) -> Result<(), RoleError> {
    if revision.version != ROLE_CONTRACT_VERSION
        || !identifier(&revision.revision)
        || revision.bindings.len() > MAX_ROLES
    {
        return Err(binding_error());
    }
    let mut roles = BTreeSet::new();
    for binding in &revision.bindings {
        if !roles.insert(&binding.role)
            || !registry.roles.iter().any(|r| r.id == binding.role)
            || [
                &binding.profile,
                &binding.profile_uuid,
                &binding.connection_generation,
                &binding.provider,
                &binding.model,
            ]
            .iter()
            .any(|s| s.is_empty() || s.len() > 1024)
            || !settings_valid(&binding.settings)
        {
            return Err(binding_error());
        }
    }
    Ok(())
}

/// Sorted JSON object keys, explicit versioning and typed arrays yield a portable identity.
pub fn canonical_identity<T: Serialize>(value: &T) -> Result<String, RoleError> {
    fn normalized(value: Value) -> Value {
        match value {
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(k, v)| (k, normalized(v)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(values) => Value::Array(values.into_iter().map(normalized).collect()),
            other => other,
        }
    }
    let value = normalized(serde_json::to_value(value).map_err(|_| invalid())?);
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&value).map_err(|_| invalid())?)
    ))
}
#[cfg(any(cargo_ai_cli, test))]
fn satisfies(requirements: &OperationRequirements, settings: &BTreeMap<String, Value>) -> bool {
    requirements.settings.iter().all(|(key, allowed)| {
        settings
            .get(key)
            .is_some_and(|value| allowed.contains(value))
    })
}
#[cfg(any(cargo_ai_cli, test))]
fn thinking(settings: &BTreeMap<String, Value>) -> ThinkingSetting {
    settings
        .get("thinking")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or(ThinkingSetting::ProviderDefault)
}

/// No I/O, provider access, persistence, grant construction or dispatch occurs here.
#[cfg(any(cargo_ai_cli, test))]
pub fn resolve(
    registry: &RoleRegistry,
    key: &ContextKey,
    revision: &BindingRevision,
    context: &ResolutionContext,
    evidence: &[CapabilityEvidence],
    now_unix_secs: u64,
) -> Result<Resolution, RoleError> {
    validate_registry(registry)?;
    validate_bindings(registry, revision)?;
    if [
        &context.project_identity,
        &context.environment_identity,
        &context.package_identity,
        &context.runtime_contract,
        &context.connection_context_identity,
        &context.consent_identity,
    ]
    .iter()
    .any(|s| s.is_empty() || s.len() > 1024)
    {
        return Err(RoleError {
            code: "role.context_unavailable",
            message: "Reliable scoped context evidence is required before resolving roles.",
        });
    }
    let scope = registry
        .contexts
        .iter()
        .find(|c| &c.key == key)
        .ok_or(RoleError {
            code: "role.unknown_context",
            message: "This action, interface and feature mode is not declared.",
        })?
        .clone();
    let mut calls = Vec::new();
    let mut selections = Vec::new();
    let mut ready = true;
    for id in &scope.call_sites {
        let site = registry.call_sites.iter().find(|s| &s.id == id).unwrap();
        let mut call = ResolvedCall {
            call_site: site.clone(),
            selection: None,
            settings: BTreeMap::new(),
            compatibility: Compatibility::Unknown,
            reason: "missing_binding".into(),
            evidence: Vec::new(),
        };
        let mut call_ready = site.role.is_none();
        if let Some(role_id) = &site.role {
            if let Some(binding) = revision.bindings.iter().find(|b| &b.role == role_id) {
                call_ready = true;
                let role = registry.roles.iter().find(|r| &r.id == role_id).unwrap();
                call.selection = Some(AllowedSelection {
                    profile: Some(binding.profile.clone()),
                    request_kind: site.kind.request_kind(),
                    model: ModelSelection::Named {
                        value: binding.model.clone(),
                    },
                    thinking: thinking(&binding.settings),
                });
                call.settings = binding.settings.clone();
                let mut comparison_missing = false;
                let mut comparison_conflict = false;
                for other in &role.different_model_from {
                    match revision.bindings.iter().find(|b| &b.role == other) {
                        Some(other) => {
                            comparison_conflict |=
                                other.provider == binding.provider && other.model == binding.model;
                        }
                        None => comparison_missing = true,
                    }
                }
                if !satisfies(&role.requirements, &binding.settings)
                    || !satisfies(&site.requirements, &binding.settings)
                    || comparison_conflict
                {
                    call_ready = false;
                    call.compatibility = Compatibility::Incompatible;
                    call.reason = "declared_constraints_conflict".into();
                } else if comparison_missing {
                    call_ready = false;
                    call.compatibility = Compatibility::Unknown;
                    call.reason = "missing_comparison_binding".into();
                } else {
                    let mut modalities = role.requirements.input_modalities.clone();
                    modalities.extend(site.requirements.input_modalities.clone());
                    modalities.sort();
                    modalities.dedup();
                    let structured =
                        role.requirements.structured_output || site.requirements.structured_output;
                    call.evidence = evidence
                        .iter()
                        .filter(|e| {
                            e.profile_uuid == binding.profile_uuid
                                && e.connection_generation == binding.connection_generation
                                && e.provider == binding.provider
                                && e.model == binding.model
                                && e.operation == site.requirements.operation
                                && e.settings == binding.settings
                                && e.structured_output == structured
                                && {
                                    let mut actual = e.input_modalities.clone();
                                    actual.sort();
                                    actual.dedup();
                                    actual == modalities
                                }
                        })
                        .cloned()
                        .collect();
                    call.reason = "capability_evidence_missing".into();
                    if !call.evidence.is_empty() {
                        call.compatibility = if call
                            .evidence
                            .iter()
                            .any(|e| e.status == Compatibility::Incompatible)
                        {
                            Compatibility::Incompatible
                        } else if call
                            .evidence
                            .iter()
                            .any(|e| e.status == Compatibility::Unavailable)
                        {
                            Compatibility::Unavailable
                        } else if call.evidence.iter().any(|e| {
                            e.status == Compatibility::Stale
                                || e.valid_until_unix_secs <= now_unix_secs
                        }) {
                            Compatibility::Stale
                        } else if call.evidence.iter().any(|e| {
                            e.status == Compatibility::Unknown
                                || e.evidence_revision.is_empty()
                                || e.provenance.is_empty()
                        }) {
                            Compatibility::Unknown
                        } else {
                            Compatibility::Compatible
                        };
                        call.reason = "exact_context_operation_evidence".into();
                    }
                }
            }
        } else if let Some(fixed) = &site.fixed {
            call.selection = Some(AllowedSelection {
                profile: fixed.profile.clone(),
                request_kind: site.kind.request_kind(),
                model: fixed.model.clone(),
                thinking: thinking(&fixed.settings),
            });
            call.settings = fixed.settings.clone();
            call.reason = "fixed_selection_requires_explicit_policy".into();
            // Fixed calls are disclosed but cannot borrow compatibility from a mapped role.
            call.compatibility = Compatibility::Unknown;
            if !satisfies(&site.requirements, &fixed.settings) {
                call_ready = false;
                call.compatibility = Compatibility::Incompatible;
                call.reason = "declared_constraints_conflict".into();
            }
        } else {
            call.compatibility = Compatibility::Compatible;
            call.reason = "structural_native_child".into();
        }
        if let Some(selection) = &call.selection {
            if !selections.contains(selection) {
                selections.push(selection.clone());
            }
        }
        ready &= call_ready;
        calls.push(call);
    }
    let mut resolution = Resolution {
        version: ROLE_CONTRACT_VERSION,
        identity: String::new(),
        binding_revision: revision.revision.clone(),
        binding_identity: canonical_identity(revision)?,
        context: context.clone(),
        scope,
        ready,
        calls,
        required_selections: selections,
        execution_authorized: false,
        invocation_access: "unverified".into(),
    };
    // Support information can change without changing the reviewed authority.
    let exact_calls: Vec<_> = resolution
        .calls
        .iter()
        .map(|call| (&call.call_site, &call.selection, &call.settings))
        .collect();
    resolution.identity = canonical_identity(&(
        ROLE_CONTRACT_VERSION,
        registry,
        revision,
        context,
        &resolution.scope,
        exact_calls,
        &resolution.required_selections,
        resolution.ready,
    ))?;
    Ok(resolution)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> (
        RoleRegistry,
        BindingRevision,
        ResolutionContext,
        Vec<CapabilityEvidence>,
    ) {
        let requirements = OperationRequirements {
            operation: "text_generation".into(),
            input_modalities: vec!["text".into()],
            structured_output: true,
            settings: BTreeMap::from([(
                "thinking".into(),
                vec![json!({"mode":"choice","value":"high"})],
            )]),
        };
        let speech = OperationRequirements {
            operation: "speech_generation".into(),
            input_modalities: vec!["text".into()],
            structured_output: false,
            settings: BTreeMap::from([
                ("voice".into(), vec![json!("coral")]),
                ("format".into(), vec![json!("wav")]),
            ]),
        };
        let role = RoleDeclaration {
            id: "reconstruction".into(),
            label: "Reconstruction".into(),
            purpose: "Create a scene".into(),
            requirements: requirements.clone(),
            different_model_from: vec![],
            recommendations: vec![],
        };
        let narration = RoleDeclaration {
            id: "narration".into(),
            label: "Narration".into(),
            purpose: "Speak a scene".into(),
            requirements: speech.clone(),
            different_model_from: vec![],
            recommendations: vec![],
        };
        let call = |id: &str, definition: &str| CallSite {
            id: id.into(),
            locator: CallLocator {
                definition: definition.into(),
                site: "root".into(),
            },
            kind: CallKind::Root,
            role: Some("reconstruction".into()),
            fixed: None,
            requirements: requirements.clone(),
            target: None,
            artifact: None,
            input_schema: None,
        };
        let mut audio = call("speech", "b.json");
        audio.kind = CallKind::Audio;
        audio.role = Some("narration".into());
        audio.locator.site = "actions.0.run.0".into();
        audio.requirements = speech;
        let scope = |action: &str, mode: &str, sites: Vec<&str>| RoleContext {
            key: ContextKey {
                action: action.into(),
                interface: action.into(),
                mode: mode.into(),
            },
            call_sites: sites.into_iter().map(String::from).collect(),
            resources: vec![format!("{action}-page")],
            data_scopes: vec![format!("{action}-data")],
            limits: ExecutionLimits {
                max_runtime_secs: 60,
                max_output_tokens: 512,
                max_agent_depth: 4,
            },
        };
        let registry = RoleRegistry {
            version: 1,
            roles: vec![role, narration],
            call_sites: vec![call("a-root", "a.json"), call("b-root", "b.json"), audio],
            contexts: vec![
                scope("a", "default", vec!["a-root"]),
                scope("b", "without-audio", vec!["b-root"]),
                scope("b", "with-audio", vec!["b-root", "speech"]),
            ],
            tool_content: BTreeMap::new(),
        };
        let binding = RoleBinding {
            role: "reconstruction".into(),
            profile: "selected".into(),
            profile_uuid: "profile-1".into(),
            connection_generation: "generation-1".into(),
            provider: "openai".into(),
            model: "gpt-5.2".into(),
            settings: BTreeMap::from([(
                "thinking".into(),
                json!({"mode":"choice","value":"high"}),
            )]),
        };
        let evidence = CapabilityEvidence {
            profile_uuid: binding.profile_uuid.clone(),
            connection_generation: binding.connection_generation.clone(),
            provider: binding.provider.clone(),
            model: binding.model.clone(),
            operation: "text_generation".into(),
            input_modalities: vec!["text".into()],
            structured_output: true,
            settings: binding.settings.clone(),
            status: Compatibility::Compatible,
            evidence_revision: "reviewed-1".into(),
            provenance: vec!["https://example.test/reviewed-model".into()],
            valid_until_unix_secs: 1000,
            operation_evidence: None,
        };
        (
            registry,
            BindingRevision {
                version: 1,
                revision: "draft-1".into(),
                bindings: vec![binding],
            },
            ResolutionContext {
                project_identity: "project-1".into(),
                environment_identity: "environment-1".into(),
                package_identity: "package-1".into(),
                runtime_contract: "native-1".into(),
                connection_context_identity: "context-1".into(),
                consent_identity: "consent-1".into(),
            },
            vec![evidence],
        )
    }
    #[test]
    fn role_contract_package_setup_keeps_action_mode_authority_separate() {
        let (registry, bindings, context, evidence) = fixture();
        let resolve_scope = |index: usize| {
            resolve(
                &registry,
                &registry.contexts[index].key,
                &bindings,
                &context,
                &evidence,
                1,
            )
            .unwrap()
        };
        let a = resolve_scope(0);
        let b = resolve_scope(1);
        let audio = resolve_scope(2);
        assert!(a.ready && b.ready);
        assert!(!audio.ready);
        assert_eq!(a.required_selections, b.required_selections);
        assert_ne!(a.identity, b.identity);
        assert_ne!(a.scope.resources, b.scope.resources);
        assert_eq!(a.calls.len(), 1);
        assert_eq!(audio.calls[1].reason, "missing_binding");
        assert!(!a.execution_authorized);
        assert_eq!(a.invocation_access, "unverified");
    }
    #[test]
    fn role_contract_recommendations_cannot_supply_bindings_evidence_or_grants() {
        let (mut registry, bindings, context, evidence) = fixture();
        let key = registry.contexts[0].key.clone();
        let original = resolve(&registry, &key, &bindings, &context, &evidence, 1).unwrap();
        let suggestion = json!({"provider":"unreviewed-provider","model":"optional-model","operation":"speech_generation","settings":{"voice":"coral"},"provenance":["https://example.test/author-notes"],"rationale":"An optional starting point, subject to the recipient's own evaluation."});
        registry.roles[0].recommendations =
            vec![serde_json::from_value(suggestion.clone()).unwrap()];
        let recommended = resolve(&registry, &key, &bindings, &context, &evidence, 1).unwrap();
        assert_eq!(recommended.calls, original.calls);
        assert_eq!(
            recommended.required_selections,
            original.required_selections
        );
        assert_eq!(recommended.scope, original.scope);
        assert!(recommended.ready);
        assert!(!recommended.execution_authorized);
        assert_ne!(
            recommended.identity, original.identity,
            "Portable edits still invalidate reviewed content identity"
        );
        let mut unmapped = bindings.clone();
        unmapped.bindings.clear();
        let unresolved = resolve(&registry, &key, &unmapped, &context, &evidence, 1).unwrap();
        assert!(!unresolved.ready);
        assert!(unresolved.calls[0].selection.is_none());
        assert!(unresolved.required_selections.is_empty());
        let unreviewed = resolve(&registry, &key, &bindings, &context, &[], 1).unwrap();
        assert!(unreviewed.ready);
        assert_eq!(unreviewed.calls[0].compatibility, Compatibility::Unknown);
        for field in [
            "profile",
            "profile_uuid",
            "connection_generation",
            "account",
            "token",
            "execution_policy",
        ] {
            let mut private = suggestion.clone();
            private[field] = json!("must not be portable");
            assert!(
                serde_json::from_value::<RoleRecommendation>(private).is_err(),
                "{field}"
            );
        }
        registry.roles[0].recommendations = vec![serde_json::from_value(suggestion).unwrap(); 9];
        assert!(validate_registry(&registry).is_err());
    }

    #[test]
    fn role_contract_support_information_preserves_readiness_and_authority_identity() {
        let (registry, bindings, context, mut evidence) = fixture();
        let key = &registry.contexts[0].key;
        let original = resolve(&registry, key, &bindings, &context, &evidence, 1).unwrap();
        for status in [
            Compatibility::Compatible,
            Compatibility::Unknown,
            Compatibility::Stale,
            Compatibility::Unavailable,
            Compatibility::Incompatible,
        ] {
            evidence[0].status = status;
            let current = resolve(&registry, key, &bindings, &context, &evidence, 1).unwrap();
            assert!(current.ready);
            assert_eq!(current.calls[0].compatibility, status);
            assert_eq!(current.identity, original.identity);
            assert_eq!(current.required_selections, original.required_selections);
            assert!(!current.execution_authorized);
            assert_eq!(current.invocation_access, "unverified");
        }
        evidence[0].status = Compatibility::Compatible;
        let expired = resolve(&registry, key, &bindings, &context, &evidence, 1000).unwrap();
        assert!(expired.ready);
        assert_eq!(expired.calls[0].compatibility, Compatibility::Stale);
        assert_eq!(expired.identity, original.identity);
        let absent = resolve(&registry, key, &bindings, &context, &[], 1).unwrap();
        assert!(absent.ready);
        assert_eq!(absent.calls[0].compatibility, Compatibility::Unknown);
        assert_eq!(absent.identity, original.identity);
    }

    #[test]
    fn role_contract_stale_and_conflicting_evidence_never_looks_compatible() {
        let (registry, bindings, context, mut evidence) = fixture();
        let key = &registry.contexts[0].key;
        assert_eq!(
            resolve(&registry, key, &bindings, &context, &evidence, 1000)
                .unwrap()
                .calls[0]
                .compatibility,
            Compatibility::Stale
        );
        let mut contradiction = evidence[0].clone();
        contradiction.status = Compatibility::Incompatible;
        evidence.push(contradiction);
        assert_eq!(
            resolve(&registry, key, &bindings, &context, &evidence, 1)
                .unwrap()
                .calls[0]
                .compatibility,
            Compatibility::Incompatible
        );
        evidence
            .iter_mut()
            .for_each(|e| e.connection_generation = "other".into());
        assert_eq!(
            resolve(&registry, key, &bindings, &context, &evidence, 1)
                .unwrap()
                .calls[0]
                .compatibility,
            Compatibility::Unknown
        );
    }
    #[test]
    fn role_contract_shared_choice_cannot_override_a_conflicting_use() {
        let (mut registry, bindings, context, evidence) = fixture();
        registry.call_sites[1].requirements.settings.insert(
            "thinking".into(),
            vec![json!({"mode":"choice","value":"low"})],
        );
        let a = resolve(
            &registry,
            &registry.contexts[0].key,
            &bindings,
            &context,
            &evidence,
            1,
        )
        .unwrap();
        let b = resolve(
            &registry,
            &registry.contexts[1].key,
            &bindings,
            &context,
            &evidence,
            1,
        )
        .unwrap();
        assert!(a.ready);
        assert!(!b.ready);
        assert_eq!(b.calls[0].compatibility, Compatibility::Incompatible);
        assert_eq!(a.calls[0].selection, b.calls[0].selection);
    }
    #[test]
    fn role_contract_identity_binds_context_choices_and_all_package_declarations() {
        let (mut registry, mut bindings, mut context, evidence) = fixture();
        let key = registry.contexts[0].key.clone();
        let original = resolve(&registry, &key, &bindings, &context, &evidence, 1)
            .unwrap()
            .identity;
        context.consent_identity = "new-consent".into();
        assert_ne!(
            original,
            resolve(&registry, &key, &bindings, &context, &evidence, 1)
                .unwrap()
                .identity
        );
        context.consent_identity = "consent-1".into();
        bindings.bindings[0].model = "different-exact-model".into();
        assert_ne!(
            original,
            resolve(&registry, &key, &bindings, &context, &evidence, 1)
                .unwrap()
                .identity
        );
        bindings.bindings[0].model = "gpt-5.2".into();
        registry.contexts[1].resources.push("new-resource".into());
        assert_ne!(
            original,
            resolve(&registry, &key, &bindings, &context, &evidence, 1)
                .unwrap()
                .identity
        );
        assert_eq!(
            canonical_identity(&json!({"b":1,"a":2})),
            canonical_identity(&json!({"a":2,"b":1}))
        );
    }
    #[test]
    fn role_contract_rejects_ambiguity_unknown_revision_and_private_diagnostics() {
        let (mut registry, bindings, context, evidence) = fixture();
        assert!(!format!("{:?}", bindings).contains("selected"));
        registry.call_sites[0].fixed = Some(FixedSelection {
            profile: None,
            model: ModelSelection::ProviderDefault {},
            settings: BTreeMap::new(),
        });
        assert!(validate_registry(&registry).is_err());
        registry.call_sites[0].fixed = None;
        registry.version = 99;
        assert_eq!(
            resolve(
                &registry,
                &registry.contexts[0].key,
                &bindings,
                &context,
                &evidence,
                1
            )
            .unwrap_err()
            .code,
            "role.unsupported_contract"
        );
    }
    #[test]
    fn role_contract_same_model_is_allowed_unless_explicitly_distinct() {
        let (mut registry, mut bindings, context, mut evidence) = fixture();
        let mut reviewer = registry.roles[0].clone();
        reviewer.id = "review".into();
        registry.roles.push(reviewer);
        registry.call_sites[1].role = Some("review".into());
        let mut binding = bindings.bindings[0].clone();
        binding.role = "review".into();
        bindings.bindings.push(binding);
        evidence.push(evidence[0].clone());
        assert!(
            resolve(
                &registry,
                &registry.contexts[1].key,
                &bindings,
                &context,
                &evidence,
                1
            )
            .unwrap()
            .ready
        );
        registry.roles[2]
            .different_model_from
            .push("reconstruction".into());
        let resolve_review = |registry: &RoleRegistry, bindings: &BindingRevision| {
            resolve(
                registry,
                &registry.contexts[1].key,
                bindings,
                &context,
                &evidence,
                1,
            )
            .unwrap()
        };
        let conflict = resolve_review(&registry, &bindings);
        assert!(!conflict.ready);
        assert_eq!(conflict.calls[0].compatibility, Compatibility::Incompatible);
        assert_eq!(conflict.calls[0].reason, "declared_constraints_conflict");

        let mut creator = bindings.bindings.remove(0);
        registry.roles[0].recommendations.push(serde_json::from_value(json!({
            "provider":creator.provider,"model":"suggested-distinct-model","operation":"text_generation",
            "provenance":["https://example.test/optional-creator"],"rationale":"An advisory model does not establish the creator binding."
        })).unwrap());
        let missing = resolve_review(&registry, &bindings);
        assert!(!missing.ready);
        assert_eq!(missing.calls[0].compatibility, Compatibility::Unknown);
        assert_eq!(missing.calls[0].reason, "missing_comparison_binding");
        assert!(missing.calls[0].selection.is_some());

        creator.model = "different-exact-model".into();
        bindings.bindings.push(creator);
        let distinct = resolve_review(&registry, &bindings);
        assert!(distinct.ready);
        assert_eq!(distinct.calls[0].compatibility, Compatibility::Compatible);
        assert_eq!(distinct.required_selections, conflict.required_selections);
        assert_eq!(distinct.scope, conflict.scope);
        assert_eq!(distinct.calls.len(), 1);
        assert_eq!(distinct.calls[0].call_site.role.as_deref(), Some("review"));
        assert!(!distinct.execution_authorized);
    }
}
