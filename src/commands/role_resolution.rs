//! Passive native role resolution. Host bindings select contexts but supply no capability proof.
use super::{
    client_actions::{self, ActionRequest},
    machine::Failure,
};
use crate::{credentials::role_context, providers::operation_metadata, role_contract};
use role_contract::{
    BindingRevision, CapabilityEvidence, Compatibility, ContextKey, Resolution, ResolutionContext,
    RoleRegistry,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

fn role_failure(error: role_contract::RoleError) -> Failure {
    Failure::new(error.code, error.message)
}
fn context_unavailable() -> Failure {
    Failure::new("role.context_unavailable", "The selected private connection context is unavailable or changed; explicitly refresh and review it.")
}
fn identity<T: Serialize>(value: &T) -> Result<String, Failure> {
    role_contract::canonical_identity(value).map_err(role_failure)
}

#[derive(Serialize)]
struct SelectedEnvironment {
    state: &'static str,
    home: PathBuf,
}

fn selected_environment(home: &Path, has_bindings: bool) -> Result<SelectedEnvironment, Failure> {
    match std::fs::canonicalize(home) {
        Ok(home) => {
            return Ok(SelectedEnvironment {
                state: "existing",
                home,
            })
        }
        Err(error) if !has_bindings && error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(context_unavailable()),
    }
    // Structural execution identifies an absent selected home without initializing it.
    let absolute = if home.is_absolute() {
        home.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| context_unavailable())?
            .join(home)
    };
    if absolute.as_os_str().len() > 4096 {
        return Err(context_unavailable());
    }
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    for ancestor in normalized.ancestors().skip(1) {
        if let Ok(existing) = std::fs::canonicalize(ancestor) {
            let suffix = normalized
                .strip_prefix(ancestor)
                .map_err(|_| context_unavailable())?;
            return Ok(SelectedEnvironment {
                state: "absent",
                home: existing.join(suffix),
            });
        }
    }
    Ok(SelectedEnvironment {
        state: "absent",
        home: normalized,
    })
}

/// Revalidate the complete package and private binding revision without repair or inference.
pub(crate) fn resolve(root: &Path, request: &ActionRequest) -> Result<Resolution, Failure> {
    let home = role_context::root().map_err(|_| context_unavailable())?;
    resolve_with_home(root, request, &home, role_context::now())
}

fn resolve_with_home(
    root: &Path,
    request: &ActionRequest,
    home: &Path,
    now: u64,
) -> Result<Resolution, Failure> {
    if request.schema_version != 3 {
        return Err(Failure::new(
            "role.unsupported_contract",
            "Native role resolution requires action request version 3.",
        ));
    }
    crate::execution_policy::ExecutionPolicy::parse(&request.execution_policy).map_err(|_| {
        Failure::new(
            "action.execution_policy_invalid",
            "The host execution policy is invalid or unsupported.",
        )
    })?;
    let execution = request.role_execution.as_ref().ok_or_else(|| {
        Failure::new(
            "role.unsupported_contract",
            "An explicit private binding revision and feature mode are required.",
        )
    })?;
    let catalog = client_actions::discover(root).map_err(client_actions::action_failure)?;
    if catalog.binding != request.expected_binding {
        return Err(Failure::new(
            "action.stale_binding",
            "The complete package action binding changed; rediscover before review.",
        ));
    }
    if !catalog.interfaces.iter().any(|interface| {
        interface.id == request.interface && interface.actions.contains(&request.action)
    }) {
        return Err(Failure::new(
            "action.unknown_action",
            "The selected interface does not declare this action.",
        ));
    }
    let registry = catalog.role_registry.as_ref().ok_or_else(|| {
        Failure::new(
            "role.unsupported_contract",
            "This catalog does not declare native roles.",
        )
    })?;
    role_contract::validate_bindings(registry, &execution.binding_revision)
        .map_err(role_failure)?;
    let key = ContextKey {
        action: request.action.clone(),
        interface: request.interface.clone(),
        mode: execution.mode.clone(),
    };
    let scope = registry
        .contexts
        .iter()
        .find(|scope| scope.key == key)
        .ok_or_else(|| {
            Failure::new(
                "role.unknown_context",
                "This action, interface and feature mode is not declared.",
            )
        })?;

    // Every supplied binding must name the exact active private generation, including shared roles.
    let profiles = validated_profiles(home, &execution.binding_revision)?;
    let fixed_selections: Vec<_> = registry
        .call_sites
        .iter()
        .filter(|site| scope.call_sites.contains(&site.id))
        .filter_map(|site| {
            site.fixed
                .as_ref()
                .map(|fixed| (site.id.clone(), fixed.profile.clone()))
        })
        .collect();
    let fixed_identity = operation_metadata::fixed_context_identity(home, &fixed_selections)
        .map_err(|_| context_unavailable())?;
    let evidence = native_evidence(
        registry,
        &scope.call_sites,
        &execution.binding_revision,
        &profiles,
        now,
    )?;
    let environment = selected_environment(home, !execution.binding_revision.bindings.is_empty())?;
    let runtime =
        crate::generated_capabilities::decode_record(&crate::CLI_RUN_RUNTIME_CAPABILITY_RECORD)
            .map_err(|_| {
                Failure::new(
                    "role.unsupported_contract",
                    "The native runtime contract cannot be qualified.",
                )
            })?;
    let grants: BTreeMap<_, _> = request
        .attachment_grants
        .iter()
        .map(|(id, grant)| {
            (
                id,
                json!({"path":grant.path,"content_sha256":grant.content_sha256}),
            )
        })
        .collect();
    let context = ResolutionContext {
        project_identity: catalog.binding.root_sha256.clone(),
        environment_identity: identity(&environment)?,
        package_identity: identity(&catalog.binding)?,
        runtime_contract: identity(&(runtime, role_contract::ROLE_CONTRACT_VERSION))?,
        connection_context_identity: operation_metadata::connection_context_identity(
            &execution.binding_revision,
            &fixed_identity,
        )
        .map_err(|_| context_unavailable())?,
        consent_identity: identity(
            &json!({"execution_policy":request.execution_policy,"attachment_grants":grants,"artifact_access":request.artifact_access,"inputs":request.inputs}),
        )?,
    };
    // A listing cannot repair stale metadata, and a change during resolution invalidates the result.
    validated_profiles(home, &execution.binding_revision)?;
    if operation_metadata::fixed_context_identity(home, &fixed_selections)
        .map_err(|_| context_unavailable())?
        != fixed_identity
    {
        return Err(context_unavailable());
    }
    client_actions::resolve_roles_from_context(
        root,
        &request.expected_binding,
        &key,
        &execution.binding_revision,
        &context,
        &evidence,
        now,
    )
    .map_err(client_actions::action_failure)
}

fn validated_profiles(
    home: &Path,
    revision: &BindingRevision,
) -> Result<BTreeMap<String, role_context::ValidatedProfileContext>, Failure> {
    let mut profiles = BTreeMap::new();
    for binding in &revision.bindings {
        let selected =
            role_context::resolve_at(home, &binding.profile_uuid, &binding.connection_generation)
                .map_err(|_| context_unavailable().with_data(json!({"role":binding.role})))?;
        let provider = crate::providers::ProviderKind::from_server_value(&selected.profile.server)
            .map(crate::providers::discovery::provider_name);
        if selected.profile_name != binding.profile || provider != Some(binding.provider.as_str()) {
            return Err(context_unavailable().with_data(json!({"role":binding.role})));
        }
        profiles.insert(binding.role.clone(), selected);
    }
    Ok(profiles)
}

fn native_evidence(
    registry: &RoleRegistry,
    active_sites: &[String],
    revision: &BindingRevision,
    profiles: &BTreeMap<String, role_context::ValidatedProfileContext>,
    now: u64,
) -> Result<Vec<CapabilityEvidence>, Failure> {
    let mut evidence = Vec::new();
    for id in active_sites {
        let site = registry
            .call_sites
            .iter()
            .find(|site| &site.id == id)
            .ok_or_else(|| {
                Failure::new(
                    "role.invalid_declaration",
                    "The native call declaration is invalid.",
                )
            })?;
        let Some(role_id) = &site.role else {
            continue;
        };
        let Some(binding) = revision
            .bindings
            .iter()
            .find(|binding| &binding.role == role_id)
        else {
            continue;
        };
        let declaration = registry
            .roles
            .iter()
            .find(|role| &role.id == role_id)
            .ok_or_else(|| {
                Failure::new(
                    "role.invalid_declaration",
                    "The native role declaration is invalid.",
                )
            })?;
        let selected = &profiles[role_id];
        let provider = crate::providers::ProviderKind::from_server_value(&selected.profile.server)
            .ok_or_else(context_unavailable)?;
        let mut inputs = declaration.requirements.input_modalities.clone();
        inputs.extend(site.requirements.input_modalities.clone());
        inputs.sort();
        inputs.dedup();
        let structured_output =
            declaration.requirements.structured_output || site.requirements.structured_output;
        let request = operation_metadata::OperationRequest {
            provider: binding.provider.clone(),
            model: binding.model.clone(),
            auth_transport: selected.profile.auth_mode.as_str().into(),
            endpoint: selected.profile.url.clone().unwrap_or_else(|| {
                if selected.profile.auth_mode
                    == crate::config::schema::ProfileAuthMode::OpenaiAccount
                {
                    crate::credentials::openai_oauth::OPENAI_ACCOUNT_RESPONSES_URL.into()
                } else {
                    provider.default_url().into()
                }
            }),
            operation: site.requirements.operation.clone(),
            input_modalities: inputs.clone(),
            structured_output,
            settings: serde_json::to_value(&binding.settings).map_err(|_| context_unavailable())?,
            catalog_presence: operation_metadata::CatalogPresence::Unknown,
            connection_available: true,
            account_metadata: None,
        };
        let assessed =
            operation_metadata::evaluate(&request, i64::try_from(now).unwrap_or(i64::MAX));
        let status = match assessed.status {
            operation_metadata::OperationStatus::Compatible => Compatibility::Compatible,
            operation_metadata::OperationStatus::Incompatible => Compatibility::Incompatible,
            operation_metadata::OperationStatus::Unknown => Compatibility::Unknown,
            operation_metadata::OperationStatus::Stale => Compatibility::Stale,
            operation_metadata::OperationStatus::Unavailable => Compatibility::Unavailable,
        };
        let provenance = assessed
            .evidence
            .iter()
            .flat_map(|record| {
                record
                    .get("sources")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(String::from)
            })
            .collect();
        let valid_until = assessed
            .evidence
            .iter()
            .filter_map(|record| record.get("expires_at_unix").and_then(Value::as_u64))
            .min()
            // Unknown/unsupported evidence has no positive validity claim or clock-dependent ID.
            .unwrap_or(u64::MAX);
        evidence.push(CapabilityEvidence {
            profile_uuid: binding.profile_uuid.clone(),
            connection_generation: binding.connection_generation.clone(),
            provider: binding.provider.clone(),
            model: binding.model.clone(),
            operation: request.operation,
            input_modalities: inputs,
            structured_output,
            settings: binding.settings.clone(),
            status,
            evidence_revision: operation_metadata::EVIDENCE_REVISION.into(),
            provenance,
            valid_until_unix_secs: valid_until,
            operation_evidence: Some(
                serde_json::to_value(&assessed).map_err(|_| context_unavailable())?,
            ),
        });
    }
    Ok(evidence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role_contract::{RoleBinding, ROLE_CONTRACT_VERSION};
    use std::{fs, path::PathBuf};

    struct Fixture {
        base: PathBuf,
        root: PathBuf,
        home: PathBuf,
        request: ActionRequest,
    }
    impl Fixture {
        fn new() -> Self {
            let base = std::env::temp_dir()
                .join(format!("cargo-ai-role-resolution-{}", uuid::Uuid::new_v4()));
            let root = base.join("project");
            let home = base.join("home");
            fs::create_dir_all(&root).unwrap();
            fs::create_dir_all(&home).unwrap();
            fs::write(home.join("config.toml"),"secret_store='file'\n[[profile]]\nname='selected'\nserver='openai'\nmodel='profile-default-is-not-role-model'\nauth_mode='api_key'\ntemperature=0.7\nthinking={mode='choice',value='low'}\n").unwrap();
            fs::write(
                home.join("credentials.toml"),
                "[profile_tokens]\nselected='synthetic-role-secret'\n",
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
                fs::set_permissions(
                    home.join("credentials.toml"),
                    fs::Permissions::from_mode(0o600),
                )
                .unwrap();
            }
            let reference = role_context::refresh_at(&home, "selected").unwrap();
            fs::write(root.join("agent.json"),serde_json::to_vec(&json!({"agent_definition_schema_version":"2026-10-03.r1","agent_schema":{"type":"object","properties":{"review":{"type":"string"}}},"actions":[]})).unwrap()).unwrap();
            let requirements = json!({"operation":"text_generation","input_modalities":["text"],"structured_output":true,"settings":{"thinking":[{"mode":"choice","value":"high"}]}});
            let use_requirements = json!({"operation":"text_generation","input_modalities":["image"],"structured_output":false,"settings":{}});
            let key = |mode: &str| json!({"action":"review","interface":"studio","mode":mode});
            let scope = |mode: &str| json!({"key":key(mode),"call_sites":["root"],"limits":{"max_runtime_secs":60,"max_output_tokens":512,"max_agent_depth":4}});
            let registry = json!({"version":1,"roles":[{"id":"reviewer","label":"Reviewer","purpose":"Review an image","requirements":requirements}],"call_sites":[{"id":"root","locator":{"definition":"agent.json","site":"root"},"kind":"root","role":"reviewer","requirements":use_requirements}],"contexts":[scope("default"),scope("detailed")],"tool_content":{}});
            role_contract::validate_registry(&serde_json::from_value(registry.clone()).unwrap())
                .unwrap();
            crate::runtime_definition::RuntimeAgentDefinition::from_str(
                &fs::read_to_string(root.join("agent.json")).unwrap(),
            )
            .unwrap();
            fs::write(root.join(client_actions::CATALOG_FILE),serde_json::to_vec(&json!({"schema_version":3,"actions":[{"id":"review","target":"agent.json","input_schema":{"type":"object","properties":{},"required":[],"additionalProperties":false},"mappings":{}}],"interfaces":[{"id":"studio","actions":["review"]}],"role_registry":registry})).unwrap()).unwrap();
            let request = ActionRequest {
                schema_version: 3,
                interface: "studio".into(),
                action: "review".into(),
                inputs: Default::default(),
                expected_binding: client_actions::discover(&root).unwrap().binding,
                execution_policy: json!({"version":1,"allowed":[],"limits":{"max_runtime_secs":60,"max_output_tokens":512,"max_agent_depth":4}}),
                attachment_grants: BTreeMap::new(),
                artifact_access: None,
                role_execution: Some(client_actions::RoleExecution {
                    binding_revision: BindingRevision {
                        version: ROLE_CONTRACT_VERSION,
                        revision: "reviewed-draft".into(),
                        bindings: vec![RoleBinding {
                            role: "reviewer".into(),
                            profile: "selected".into(),
                            profile_uuid: reference.profile_uuid,
                            connection_generation: reference.connection_generation,
                            provider: "openai".into(),
                            model: "gpt-5.2".into(),
                            settings: BTreeMap::from([(
                                "thinking".into(),
                                json!({"mode":"choice","value":"high"}),
                            )]),
                        }],
                    },
                    resolution_id: None,
                    mode: "default".into(),
                }),
            };
            Self {
                base,
                root,
                home,
                request,
            }
        }
        fn resolve(&self) -> Result<Resolution, Failure> {
            resolve_with_home(&self.root, &self.request, &self.home, role_context::now())
        }
        fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
            [&self.root, &self.home]
                .into_iter()
                .flat_map(|root| {
                    fs::read_dir(root)
                        .unwrap()
                        .map(|entry| entry.unwrap().path())
                })
                .filter(|path| path.is_file())
                .map(|path| {
                    let bytes = fs::read(&path).unwrap();
                    (path, bytes)
                })
                .collect()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    #[test]
    fn native_fixed_selections_detect_secret_drift_and_actual_default_mapping() {
        let mut fixture = Fixture::new();
        let mut catalog: Value = serde_json::from_slice(
            &fs::read(fixture.root.join(client_actions::CATALOG_FILE)).unwrap(),
        )
        .unwrap();
        let site = &mut catalog["role_registry"]["call_sites"][0];
        site.as_object_mut().unwrap().remove("role");
        site["fixed"] = json!({"profile":"selected","model":{"kind":"named","value":"gpt-5.2"},"settings":{"thinking":{"mode":"choice","value":"high"}}});
        catalog["role_registry"]["roles"] = json!([]);
        fs::write(
            fixture.root.join(client_actions::CATALOG_FILE),
            serde_json::to_vec(&catalog).unwrap(),
        )
        .unwrap();
        fixture.request.expected_binding = client_actions::discover(&fixture.root).unwrap().binding;
        fixture
            .request
            .role_execution
            .as_mut()
            .unwrap()
            .binding_revision
            .bindings
            .clear();
        let before = fixture.snapshot();
        let named = fixture.resolve().unwrap();
        assert!(named.ready);
        assert!(!named.execution_authorized);
        assert_eq!(named.calls[0].compatibility, Compatibility::Unknown);
        assert!(named.calls[0].call_site.role.is_none());
        assert_eq!(before, fixture.snapshot());
        let config_before = fs::read(fixture.home.join("config.toml")).unwrap();
        fs::write(fixture.home.join("credentials.toml"), "[profile_tokens]\nselected='changed-synthetic-secret'\nother='second-synthetic-secret'\n").unwrap();
        let after_secret = fixture.snapshot();
        assert_eq!(
            fixture.resolve().unwrap_err().code,
            "role.context_unavailable"
        );
        assert_eq!(
            config_before,
            fs::read(fixture.home.join("config.toml")).unwrap()
        );
        assert_eq!(after_secret, fixture.snapshot());
        role_context::refresh_at(&fixture.home, "selected").unwrap();
        let refreshed = fixture.resolve().unwrap();
        assert_ne!(named.identity, refreshed.identity);
        assert_eq!(refreshed.calls[0].compatibility, Compatibility::Unknown);

        fs::write(fixture.home.join("config.toml"), "secret_store='file'\ndefault_profile='selected'\n[[profile]]\nname='selected'\nserver='openai'\nmodel='default-selected'\nauth_mode='api_key'\n[[profile]]\nname='other'\nserver='openai'\nmodel='default-other'\nauth_mode='api_key'\n").unwrap();
        role_context::refresh_at(&fixture.home, "selected").unwrap();
        role_context::refresh_at(&fixture.home, "other").unwrap();
        catalog["role_registry"]["call_sites"][0]["fixed"]["profile"] = Value::Null;
        fs::write(
            fixture.root.join(client_actions::CATALOG_FILE),
            serde_json::to_vec(&catalog).unwrap(),
        )
        .unwrap();
        fixture.request.expected_binding = client_actions::discover(&fixture.root).unwrap().binding;
        let default_selected = fixture.resolve().unwrap();
        let config = fs::read_to_string(fixture.home.join("config.toml")).unwrap();
        fs::write(
            fixture.home.join("config.toml"),
            config.replace("default_profile='selected'", "default_profile='other'"),
        )
        .unwrap();
        assert_eq!(
            fixture.resolve().unwrap_err().code,
            "role.context_unavailable"
        );
        role_context::refresh_at(&fixture.home, "other").unwrap();
        let after_refresh = fixture.snapshot();
        let default_other = fixture.resolve().unwrap();
        assert_ne!(
            default_selected.context.connection_context_identity,
            default_other.context.connection_context_identity
        );
        assert!(default_other.calls[0]
            .selection
            .as_ref()
            .unwrap()
            .profile
            .is_none());
        assert!(default_other.calls[0].call_site.role.is_none());
        assert_eq!(after_refresh, fixture.snapshot());
        let public = serde_json::to_string(&default_other).unwrap();
        assert!(!public.contains("synthetic-secret"));
    }

    #[test]
    fn native_structural_resolution_does_not_initialize_an_absent_selected_home() {
        let mut fixture = Fixture::new();
        fs::write(
            fixture.root.join("review.json"),
            fs::read(fixture.root.join("agent.json")).unwrap(),
        )
        .unwrap();
        let mut definition = json!({"agent_definition_schema_version":"2026-10-03.r1","agent_schema":{"type":"object","properties":{}},"actions":[]});
        fs::write(
            fixture.root.join("child.json"),
            serde_json::to_vec(&definition).unwrap(),
        )
        .unwrap();
        definition["actions"] = json!([{"name":"launch","logic":{"==":[1,1]},"run":[{"kind":"agent","artifact":"./child.json"}]}]);
        fs::write(
            fixture.root.join("agent.json"),
            serde_json::to_vec(&definition).unwrap(),
        )
        .unwrap();
        let mut catalog: Value = serde_json::from_slice(
            &fs::read(fixture.root.join(client_actions::CATALOG_FILE)).unwrap(),
        )
        .unwrap();
        let mut other_root = catalog["role_registry"]["call_sites"][0].clone();
        other_root["id"] = json!("other-root");
        other_root["locator"]["definition"] = json!("review.json");
        catalog["role_registry"]["call_sites"] = json!([{"id":"launch","locator":{"definition":"agent.json","site":"actions.0.run.0"},"kind":"child","target":"child.json","requirements":{"operation":"text_generation"}}, other_root]);
        for scope in catalog["role_registry"]["contexts"].as_array_mut().unwrap() {
            scope["call_sites"] = json!(["launch"]);
        }
        catalog["actions"].as_array_mut().unwrap().push(json!({"id":"other-review","target":"review.json","input_schema":{"type":"object","properties":{},"required":[],"additionalProperties":false},"mappings":{}}));
        catalog["interfaces"][0]["actions"]
            .as_array_mut()
            .unwrap()
            .push(json!("other-review"));
        catalog["role_registry"]["contexts"].as_array_mut().unwrap().push(json!({"key":{"action":"other-review","interface":"studio","mode":"default"},"call_sites":["other-root"],"limits":{"max_runtime_secs":60,"max_output_tokens":512,"max_agent_depth":4}}));
        fs::write(
            fixture.root.join(client_actions::CATALOG_FILE),
            serde_json::to_vec(&catalog).unwrap(),
        )
        .unwrap();
        fixture.request.expected_binding = client_actions::discover(&fixture.root).unwrap().binding;
        fixture
            .request
            .role_execution
            .as_mut()
            .unwrap()
            .binding_revision
            .bindings
            .clear();
        let absent = fixture.base.join("absent-parent/home");
        let before = fixture.snapshot();
        let first = resolve_with_home(
            &fixture.root,
            &fixture.request,
            &absent,
            role_context::now(),
        )
        .unwrap();
        assert!(first.ready);
        assert!(!first.execution_authorized);
        assert_eq!(first.calls[0].reason, "structural_native_child");
        assert!(first.calls[0].selection.is_none());
        assert!(first.required_selections.is_empty());
        assert!(!absent.parent().unwrap().exists());
        assert_eq!(before, fixture.snapshot());
        let normalized_alias = fixture.base.join("absent-parent/unused/../home");
        let alias = resolve_with_home(
            &fixture.root,
            &fixture.request,
            &normalized_alias,
            role_context::now(),
        )
        .unwrap();
        assert_eq!(
            first.context.environment_identity,
            alias.context.environment_identity
        );
        assert!(!absent.parent().unwrap().exists());
        fs::create_dir_all(&absent).unwrap();
        let existing = resolve_with_home(
            &fixture.root,
            &fixture.request,
            &absent,
            role_context::now(),
        )
        .unwrap();
        assert!(existing.ready);
        assert_ne!(first.identity, existing.identity);
        assert_eq!(fs::read_dir(&absent).unwrap().count(), 0);
        assert_eq!(before, fixture.snapshot());
    }

    #[test]
    fn installed_native_role_example_is_a_verified_portable_catalog() {
        let canonical =
            include_str!("../../templates/guidance/client-actions.md").replace("\r\n", "\n");
        for guidance in [canonical.clone(), canonical.replace('\n', "\r\n")] {
            let fixture = Fixture::new();
            let guidance = guidance.replace("\r\n", "\n");
            let example = guidance
                .split("```json\n")
                .skip(1)
                .filter_map(|section| section.split_once("```"))
                .map(|(json, _)| json)
                .find(|json| json.contains("\"schema_version\": 3"))
                .expect("Installed guidance includes the native catalog example");
            fs::write(fixture.root.join(client_actions::CATALOG_FILE), example).unwrap();
            let catalog = client_actions::discover(&fixture.root).unwrap();
            assert_eq!(catalog.schema_version, 3);
            let registry = catalog.role_registry.unwrap();
            assert_eq!(registry.call_sites[0].id, "review-root");
            assert_eq!(registry.contexts[0].key.action, "review");
        }
    }

    #[test]
    fn native_resolution_uses_production_evidence_and_exact_context_without_state_writes() {
        let fixture = Fixture::new();
        let before = fixture.snapshot();
        let resolution = fixture.resolve().unwrap();
        assert!(resolution.ready);
        assert!(!resolution.execution_authorized);
        assert_eq!(resolution.invocation_access, "unverified");
        assert_eq!(resolution.calls.len(), 1);
        let call = &resolution.calls[0];
        assert_eq!(call.compatibility, Compatibility::Compatible);
        assert_eq!(call.evidence[0].input_modalities, vec!["image", "text"]);
        assert!(call.evidence[0].structured_output);
        assert_eq!(call.evidence[0].provider, "openai");
        assert_eq!(call.evidence[0].model, "gpt-5.2");
        assert_eq!(
            call.evidence[0].evidence_revision,
            operation_metadata::EVIDENCE_REVISION
        );
        assert_eq!(
            call.evidence[0].provenance,
            vec!["https://developers.openai.com/api/docs/models/gpt-5.2"]
        );
        let details = call.evidence[0].operation_evidence.as_ref().unwrap();
        assert_eq!(details["auth_transport"], "api_key");
        assert_eq!(
            details["endpoint_family"],
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(details["invocation_access"], "unverified");
        assert_eq!(details["evidence"][0]["reviewed_at"], "2026-10-06");
        assert_eq!(resolution.identity, fixture.resolve().unwrap().identity);
        assert_eq!(before, fixture.snapshot());
        let encoded = serde_json::to_string(&resolution).unwrap();
        assert!(!encoded.contains("synthetic-role-secret"));
        assert!(!encoded.contains(&fixture.home.to_string_lossy().to_string()));
        assert!(!encoded.contains("profile-default-is-not-role-model"));
    }

    #[test]
    fn native_resolution_invalidates_exact_profile_provider_generation_and_package_changes() {
        for field in [
            "profile",
            "provider",
            "profile_uuid",
            "connection_generation",
        ] {
            let mut fixture = Fixture::new();
            let binding = &mut fixture
                .request
                .role_execution
                .as_mut()
                .unwrap()
                .binding_revision
                .bindings[0];
            match field {
                "profile" => binding.profile = "other".into(),
                "provider" => binding.provider = "gemini".into(),
                "profile_uuid" => binding.profile_uuid = "missing".into(),
                _ => binding.connection_generation = "missing".into(),
            }
            let before = fixture.snapshot();
            assert_eq!(
                fixture.resolve().unwrap_err().code,
                "role.context_unavailable"
            );
            assert_eq!(before, fixture.snapshot());
        }
        let fixture = Fixture::new();
        let definition = fixture.root.join("agent.json");
        let mut bytes = fs::read(&definition).unwrap();
        bytes.push(b'\n');
        fs::write(&definition, bytes).unwrap();
        assert_eq!(fixture.resolve().unwrap_err().code, "action.stale_binding");
        let fixture = Fixture::new();
        let path = fixture.home.join("credentials.toml");
        let current = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            current.replace("synthetic-role-secret", "changed-synthetic-secret"),
        )
        .unwrap();
        let before = fixture.snapshot();
        assert_eq!(
            fixture.resolve().unwrap_err().code,
            "role.context_unavailable"
        );
        assert_eq!(before, fixture.snapshot());
    }

    #[test]
    fn native_resolution_scopes_consent_feature_mode_environment_and_evidence_freshness() {
        let mut fixture = Fixture::new();
        let first = fixture.resolve().unwrap();
        fixture.request.execution_policy["limits"]["max_output_tokens"] = json!(256);
        let changed = fixture.resolve().unwrap();
        assert_ne!(first.identity, changed.identity);
        assert_ne!(
            first.context.consent_identity,
            changed.context.consent_identity
        );
        assert!(!changed.execution_authorized);
        fixture.request.role_execution.as_mut().unwrap().mode = "detailed".into();
        assert_ne!(changed.identity, fixture.resolve().unwrap().identity);
        fixture.request.role_execution.as_mut().unwrap().mode = "unknown".into();
        assert_eq!(fixture.resolve().unwrap_err().code, "role.unknown_context");
        fixture.request.role_execution.as_mut().unwrap().mode = "default".into();
        let expired = resolve_with_home(
            &fixture.root,
            &fixture.request,
            &fixture.home,
            1_800_000_000,
        )
        .unwrap();
        assert!(!expired.ready);
        assert_eq!(expired.calls[0].compatibility, Compatibility::Stale);
        let second = Fixture::new();
        let other = second.resolve().unwrap();
        assert_ne!(
            first.context.environment_identity,
            other.context.environment_identity
        );
        assert_ne!(
            first.context.project_identity,
            other.context.project_identity
        );
    }

    #[test]
    fn native_resolution_cannot_use_catalog_or_host_optimism_for_unknown_model_settings() {
        let mut fixture = Fixture::new();
        fixture
            .request
            .role_execution
            .as_mut()
            .unwrap()
            .binding_revision
            .bindings[0]
            .model = "catalog-only-unqualified".into();
        let result = fixture.resolve().unwrap();
        assert!(!result.ready);
        assert_eq!(result.calls[0].compatibility, Compatibility::Unknown);
        assert_eq!(result.identity, fixture.resolve().unwrap().identity);
        assert!(!result.execution_authorized);
        let raw = json!({"schema_version":3,"interface":"studio","action":"review","inputs":{},"expected_binding":fixture.request.expected_binding,"execution_policy":fixture.request.execution_policy,"role_execution":{"binding_revision":fixture.request.role_execution.as_ref().unwrap().binding_revision,"mode":"default"},"evidence":[{"status":"compatible"}]});
        assert!(client_actions::parse_request(&raw.to_string()).is_err());
        let missing = fixture.home.join("absent-home");
        assert!(resolve_with_home(
            &fixture.root,
            &fixture.request,
            &missing,
            role_context::now()
        )
        .is_err());
        assert!(!missing.exists());
        fixture.request.execution_policy = json!({"version":1,"allowed":[],"limits":{"max_runtime_secs":0,"max_output_tokens":1,"max_agent_depth":0}});
        assert_eq!(
            fixture.resolve().unwrap_err().code,
            "action.execution_policy_invalid"
        );
    }
}
