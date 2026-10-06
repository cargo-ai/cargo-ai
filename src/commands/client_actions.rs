//! Declared actions, business inputs and authorized resources for application callers.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub(crate) const CATALOG_FILE: &str = "cargo-ai-actions.json";
const MAX_CATALOG_BYTES: usize = 256 * 1024;
const MAX_DEFINITION_BYTES: usize = 1024 * 1024;
const MAX_RESOURCE_BYTES: usize = 4 * 1024 * 1024;
const MAX_INVENTORY_BYTES: usize = 16 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
const MAX_MEMBERS: usize = 64;
const MAX_DEPTH: usize = 8;

#[derive(Clone, Debug)]
pub(crate) struct ActionError {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
}
impl ActionError {
    pub(crate) fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
    fn catalog(_: impl std::fmt::Display) -> Self {
        Self::new(
            "action.invalid_catalog",
            "The declared action catalog is invalid.",
        )
    }
    fn inputs(_: impl std::fmt::Display) -> Self {
        Self::new(
            "action.invalid_inputs",
            "The action request contains invalid inputs or mappings.",
        )
    }
}
impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl From<String> for ActionError {
    fn from(value: String) -> Self {
        Self::catalog(value)
    }
}
impl From<&str> for ActionError {
    fn from(value: &str) -> Self {
        Self::catalog(value)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActionDocument {
    pub(crate) id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    pub(crate) target: String,
    pub(crate) input_schema: Value,
    pub(crate) mappings: BTreeMap<String, InputMapping>,
    #[serde(default)]
    pub(crate) constants: ActionConstants,
    #[serde(default)]
    pub(crate) required_capabilities: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum InputMapping {
    RuntimeVar(RuntimeVarMapping),
    Input(NamedInputMapping),
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeVarMapping {
    pub(crate) runtime_var: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) encoding: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NamedInputMapping {
    pub(crate) input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) encoding: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActionConstants {
    #[serde(default)]
    pub(crate) runtime_vars: BTreeMap<String, Value>,
    #[serde(default)]
    pub(crate) inputs: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InterfaceDocument {
    pub(crate) id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    pub(crate) actions: Vec<String>,
    #[serde(default)]
    pub(crate) resources: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) presentation: Option<PresentationDocument>,
    #[serde(default)]
    pub(crate) artifact_scopes: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResourceDocument {
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) mime_type: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PresentationDocument {
    pub(crate) entrypoint: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CatalogDocument {
    schema_version: u32,
    actions: Vec<ActionDocument>,
    interfaces: Vec<InterfaceDocument>,
    #[serde(default)]
    resources: Vec<ResourceDocument>,
    #[serde(default)]
    artifact_scopes: Vec<super::action_artifacts::ArtifactScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    role_registry: Option<crate::role_contract::RoleRegistry>,
}

/// Opaque content identity. A source root, target, or resource change invalidates it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActionBinding {
    pub(crate) root_sha256: String,
    pub(crate) catalog_sha256: String,
    pub(crate) content_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) package: Option<PackageBinding>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PackageBinding {
    pub(crate) alias: String,
    pub(crate) version: String,
    pub(crate) content_sha256: String,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ActionCatalog {
    pub(crate) schema_version: u32,
    pub(crate) limits: Value,
    pub(crate) capabilities: Vec<&'static str>,
    pub(crate) binding: ActionBinding,
    pub(crate) actions: Vec<Value>,
    pub(crate) resources: Vec<ResourceDocument>,
    pub(crate) interfaces: Vec<InterfaceDocument>,
    pub(crate) artifact_scopes: Vec<super::action_artifacts::ArtifactScope>,
    pub(crate) supported_catalog_versions: Vec<u32>,
    pub(crate) supported_request_versions: Vec<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) role_registry: Option<crate::role_contract::RoleRegistry>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RoleExecution {
    pub(crate) binding_revision: crate::role_contract::BindingRevision,
    #[serde(default)]
    pub(crate) resolution_id: Option<String>,
    pub(crate) mode: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActionRequest {
    pub(crate) schema_version: u32,
    pub(crate) interface: String,
    pub(crate) action: String,
    pub(crate) inputs: Map<String, Value>,
    pub(crate) expected_binding: ActionBinding,
    pub(crate) execution_policy: Value,
    #[serde(default)]
    pub(crate) attachment_grants: BTreeMap<String, AttachmentGrant>,
    #[serde(default)]
    pub(crate) artifact_access: Option<super::action_artifacts::Permission>,
    #[serde(default)]
    pub(crate) role_execution: Option<RoleExecution>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AttachmentGrant {
    pub(crate) path: PathBuf,
    pub(crate) content_sha256: String,
}
#[derive(Clone, Debug)]
pub(crate) struct PreparedAction {
    pub(crate) interface: String,
    pub(crate) project_root: PathBuf,
    pub(crate) action: String,
    pub(crate) binding: ActionBinding,
    pub(crate) definition_path: PathBuf,
    pub(crate) definition_json: String,
    pub(crate) run_vars: Vec<String>,
    pub(crate) input_overrides: Vec<String>,
    pub(crate) installed: Option<super::local_packages::CheckedInstalledPackageRuntime>,
    pub(crate) execution_policy: Value,
    pub(crate) artifact_context: Option<super::action_artifacts::Context>,
    pub(crate) role_execution: Option<RoleExecution>,
}
struct LoadedCatalog {
    root: PathBuf,
    document: CatalogDocument,
    binding: ActionBinding,
    files: BTreeMap<String, Vec<u8>>,
    installed: Option<super::local_packages::CheckedInstalledPackageRuntime>,
}

#[derive(Clone, Debug)]
pub(crate) struct ActionTarget {
    pub(crate) root: PathBuf,
    pub(crate) _installed: Option<super::local_packages::CheckedInstalledPackageRuntime>,
}
pub(crate) fn load_target(
    project: Option<&Path>,
    package: Option<&str>,
) -> Result<ActionTarget, ActionError> {
    match (project, package) {
        (Some(root), None) => {
            let metadata = fs::symlink_metadata(root)
                .map_err(|e| format!("Cannot inspect action project root: {e}"))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err("Action project root must be a real directory.".into());
            }
            let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
            let installed = super::local_packages::checked_runtime_lease_for_path(&root, None)?;
            if let Some(checked) = &installed {
                let current = std::env::current_dir().map_err(|error| error.to_string())?;
                if let Some(project) = super::package_dependencies::find_project_root(&current)? {
                    if fs::canonicalize(&project).ok()
                        != fs::canonicalize(&checked.context.package_payload_root).ok()
                    {
                        super::local_packages::validate_installed_alias_dependency_for_project(
                            &checked.context.alias,
                            &project,
                        )?;
                    }
                }
            }
            Ok(ActionTarget {
                root,
                _installed: installed,
            })
        }
        (None, Some(alias)) => {
            let current = std::env::current_dir().map_err(|e| e.to_string())?;
            let project = super::package_dependencies::find_project_root(&current)?;
            let installed =
                super::local_packages::checked_action_target_for_alias(alias, project.as_deref())?;
            Ok(ActionTarget {
                root: installed.context.package_payload_root.clone(),
                _installed: Some(installed),
            })
        }
        _ => Err("Choose exactly one action project or installed package alias.".into()),
    }
}
pub(crate) fn read_request_stdin() -> Result<String, String> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Cannot read action request: {e}"))?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err("Action request exceeds the 64 KiB limit.".into());
    }
    String::from_utf8(bytes).map_err(|_| "Action request must be UTF-8".into())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceRequest {
    schema_version: u32,
    interface: String,
    resource: String,
    expected_binding: ActionBinding,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactReadRequest {
    schema_version: u32,
    interface: String,
    expected_binding: ActionBinding,
    reference: String,
    read_grant: super::action_artifacts::ReadGrant,
}

fn artifact_error(error: super::action_artifacts::ArtifactError) -> ActionError {
    ActionError::new(error.code, error.message)
}

fn artifact_context(
    loaded: &LoadedCatalog,
    interface: &InterfaceDocument,
    allowed_scopes: Vec<String>,
) -> Result<super::action_artifacts::Context, ActionError> {
    let (data_root, identity) = if let Some(installed) = &loaded.installed {
        let receipt_identity = super::local_packages::artifact_installation_identity(installed)
            .map_err(|_| {
                ActionError::new(
                    "artifact.stale_context",
                    "The installed artifact context could not be verified.",
                )
            })?;
        (
            installed.context.package_data_root.clone(),
            json!({"installation":receipt_identity,"root":installed.context.package_data_root}),
        )
    } else {
        let config = super::action_artifacts::read_owned_bytes(
            &loaded.root,
            ".cargo-ai/project.toml",
            MAX_DEFINITION_BYTES,
        )
        .map_err(|_| {
            ActionError::new(
                "artifact.stale_context",
                "The project artifact context could not be verified.",
            )
        })?;
        let metadata = std::str::from_utf8(&config).ok().ok_or_else(|| {
            ActionError::new(
                "artifact.stale_context",
                "The project artifact context could not be verified.",
            )
        })?;
        if !super::runtime_data::uses_project_data(metadata).map_err(|_| {
            ActionError::new(
                "artifact.stale_context",
                "The project artifact context could not be verified.",
            )
        })? {
            return Err(ActionError::new(
                "artifact.access_denied",
                "Artifact scopes require explicit project runtime-data adoption.",
            ));
        }
        let path = loaded.root.join(super::runtime_data::PROJECT_DATA_PATH);
        (
            path,
            json!({"project":loaded.root,"configuration":sha256(&config)}),
        )
    };
    let mut identity_bytes = b"cargo-ai.artifact-data-context.v1\0".to_vec();
    identity_bytes.extend(serde_json::to_vec(&identity).map_err(ActionError::catalog)?);
    Ok(super::action_artifacts::Context {
        data_root,
        data_context_sha256: sha256(&identity_bytes),
        interface: interface.id.clone(),
        binding: serde_json::to_value(&loaded.binding).map_err(ActionError::catalog)?,
        scopes: loaded.document.artifact_scopes.clone(),
        allowed_scopes,
    })
}

tokio::task_local! {
    static RESULT_ARTIFACT_CONTEXT: Option<super::action_artifacts::Context>;
}

pub(crate) async fn scope_result_artifacts<F: std::future::Future>(
    context: Option<super::action_artifacts::Context>,
    future: F,
) -> F::Output {
    RESULT_ARTIFACT_CONTEXT.scope(context, future).await
}

pub(crate) fn current_result_artifact_context() -> Option<super::action_artifacts::Context> {
    RESULT_ARTIFACT_CONTEXT
        .try_with(Clone::clone)
        .ok()
        .flatten()
}

pub(crate) fn validate_result_artifact_scopes(scopes: &[String]) -> Result<(), String> {
    if scopes.is_empty() {
        return Ok(());
    }
    let context = current_result_artifact_context().ok_or_else(|| {
        "artifact.access_denied: A declared action and native artifact permission are required."
            .to_string()
    })?;
    if scopes
        .iter()
        .any(|scope| !context.allowed_scopes.contains(scope))
    {
        return Err(
            "artifact.access_denied: Producer scopes exceed the authorized action scopes.".into(),
        );
    }
    Ok(())
}

pub(crate) fn export_result_artifacts(
    nominations: &[crate::business_schema::ArtifactNomination],
    scopes: &[String],
) -> Result<(), String> {
    if nominations.is_empty() {
        return Ok(());
    }
    let result = (|| {
        let mut context = current_result_artifact_context().ok_or_else(|| {
            ActionError::new(
                "artifact.access_denied",
                "Artifact export requires a declared authorized action.",
            )
        })?;
        validate_result_artifact_scopes(scopes).map_err(|_| {
            ActionError::new(
                "artifact.access_denied",
                "Artifact producer scopes are not authorized.",
            )
        })?;
        context.allowed_scopes.retain(|id| scopes.contains(id));
        super::action_artifacts::issue(&context, nominations).map_err(artifact_error)
    })();
    match result {
        Ok((references, grants)) => {
            super::machine::record_result_artifacts(json!(references), json!(grants));
            Ok(())
        }
        Err(_) => {
            let message = "Artifact export failed; committed business effects may remain. No artifact grants were issued.";
            super::machine::record_error(
                super::machine::Failure::new("artifact.export_failed", message)
                    .with_data(json!({"partial":true})),
            );
            Err(format!("artifact.export_failed: {message}"))
        }
    }
}

pub(crate) fn machine_run(matches: &clap::ArgMatches) -> Result<Value, super::machine::Failure> {
    let (command, args) = matches.subcommand().ok_or_else(|| {
        super::machine::Failure::new("cli.invalid_input", "Missing actions command.")
    })?;
    let target = load_target(
        args.get_one::<String>("project").map(Path::new),
        args.get_one::<String>("package").map(String::as_str),
    )
    .map_err(action_failure)?;
    match command {
        "list" => {
            serde_json::to_value(discover(&target.root).map_err(action_failure)?).map_err(|_| {
                super::machine::Failure::new(
                    "cli.serialization_failed",
                    "Could not serialize action catalog.",
                )
            })
        }
        "validate" | "resolve" => {
            let raw =
                read_request_stdin().map_err(|error| action_failure(ActionError::inputs(error)))?;
            let request = parse_request(&raw).map_err(action_failure)?;
            if args.get_one::<String>("interface") != Some(&request.interface)
                || args.get_one::<String>("action") != Some(&request.action)
            {
                return Err(super::machine::Failure::new(
                    "action.invalid_request",
                    "Action request selectors do not match the CLI selectors.",
                ));
            }
            if command == "resolve" {
                let resolution = super::role_resolution::resolve(&target.root, &request)?;
                return serde_json::to_value(resolution).map_err(|_| {
                    super::machine::Failure::new(
                        "cli.serialization_failed",
                        "Could not serialize role resolution.",
                    )
                });
            }
            let prepared = prepare(&target.root, &request).map_err(action_failure)?;
            Ok(
                json!({"schema_version":request.schema_version,"valid":true,"execution_authorized":false,"interface":prepared.interface,"action":prepared.action,"binding":prepared.binding,
                "mappings":{"runtime_vars":prepared.run_vars.iter().filter_map(|value|value.split_once('=').map(|(name,_)|name)).collect::<Vec<_>>(),"inputs":prepared.input_overrides.iter().filter_map(|value|value.split_once('=').map(|(name,_)|name)).collect::<Vec<_>>()}}),
            )
        }
        "artifact" => {
            let raw = read_request_stdin().map_err(|_| {
                super::machine::Failure::new(
                    "artifact.invalid_request",
                    "Invalid bounded artifact request.",
                )
            })?;
            let value =
                crate::business_schema::strict_json_bounded(raw.as_bytes(), MAX_REQUEST_BYTES, 32)
                    .map_err(|_| {
                        super::machine::Failure::new(
                            "artifact.invalid_request",
                            "Invalid artifact request JSON.",
                        )
                    })?;
            let request: ArtifactReadRequest = serde_json::from_value(value).map_err(|_| {
                super::machine::Failure::new(
                    "artifact.invalid_request",
                    "Invalid artifact request.",
                )
            })?;
            if request.schema_version != 1
                || args.get_one::<String>("interface") != Some(&request.interface)
            {
                return Err(super::machine::Failure::new(
                    "artifact.invalid_request",
                    "Artifact selectors or request revision do not match.",
                ));
            }
            let loaded = load_catalog(&target.root).map_err(action_failure)?;
            if loaded.binding != request.expected_binding {
                return Err(super::machine::Failure::new(
                    "artifact.stale_binding",
                    "Artifact binding changed; obtain a fresh reference.",
                ));
            }
            let interface = loaded
                .document
                .interfaces
                .iter()
                .find(|interface| interface.id == request.interface)
                .ok_or_else(|| {
                    super::machine::Failure::new(
                        "artifact.access_denied",
                        "The artifact interface is unavailable.",
                    )
                })?;
            let context = artifact_context(&loaded, interface, interface.artifact_scopes.clone())
                .map_err(action_failure)?;
            // Both the target and catalog leases remain held until verified bytes are returned.
            super::action_artifacts::read(&context, &request.read_grant, &request.reference)
                .map_err(|error| super::machine::Failure::new(error.code, error.message))
        }
        "resource" => {
            let raw =
                read_request_stdin().map_err(|error| action_failure(ActionError::inputs(error)))?;
            let value: Value = strict_json(raw.as_bytes()).map_err(|_| {
                super::machine::Failure::new(
                    "action.invalid_request",
                    "Invalid action resource request.",
                )
            })?;
            validate_envelope_bounds(&value, 0)
                .map_err(|error| action_failure(ActionError::inputs(error)))?;
            let request: ResourceRequest = serde_json::from_value(value).map_err(|_| {
                super::machine::Failure::new(
                    "action.invalid_request",
                    "Invalid action resource request.",
                )
            })?;
            if request.schema_version != 1
                || args.get_one::<String>("interface") != Some(&request.interface)
                || args.get_one::<String>("resource") != Some(&request.resource)
            {
                return Err(super::machine::Failure::new(
                    "action.invalid_request",
                    "Resource request selectors or version do not match.",
                ));
            }
            resource(
                &target.root,
                &request.interface,
                &request.resource,
                &request.expected_binding,
            )
            .map_err(action_failure)
        }
        _ => Err(super::machine::Failure::new(
            "cli.invalid_input",
            "Unsupported actions command.",
        )),
    }
}
pub(crate) fn action_failure(error: ActionError) -> super::machine::Failure {
    super::machine::Failure::new(error.code, error.message)
}

pub(crate) fn limits() -> Value {
    json!({"catalog_bytes":MAX_CATALOG_BYTES,"actions":256,"interfaces":64,"resources":64,"request_bytes":MAX_REQUEST_BYTES,"business_depth":MAX_DEPTH,"array_items":1024,"object_members":MAX_MEMBERS,"resource_bytes":MAX_RESOURCE_BYTES,"inventory_bytes":MAX_INVENTORY_BYTES,"definition_bytes":MAX_DEFINITION_BYTES,"attachment_bytes":MAX_ATTACHMENT_BYTES,"string_bytes":64*1024,"business_key_bytes":128,"result_bytes":65536,"producer_payload_bytes":131072,"artifact":{"read_types":super::action_artifacts::SUPPORTED_TYPES,"file_bytes":super::action_artifacts::MAX_ARTIFACT_BYTES,"request_bytes":super::action_artifacts::MAX_REQUEST_BYTES,"nominations":64,"format_chunks":4096,"json_depth":32,"json_nodes":65536,"response_bytes":8*1024*1024,"native_confinement":cfg!(any(target_os="linux",target_os="macos",windows)),"authorization":"trusted_host_grant"}})
}
pub(crate) fn discover(root: &Path) -> Result<ActionCatalog, ActionError> {
    let loaded = load_catalog(root)?;
    let actions = loaded
        .document
        .actions
        .iter()
        .map(|action| {
            let mut value = serde_json::to_value(action).map_err(ActionError::catalog)?;
            let definition =
                strict_json(&loaded.files[&action.target]).map_err(ActionError::catalog)?;
            value["result"] = definition.get("result").cloned().unwrap_or(Value::Null);
            Ok(value)
        })
        .collect::<Result<Vec<_>, ActionError>>()?;
    Ok(ActionCatalog {
        schema_version: loaded.document.schema_version,
        limits: limits(),
        capabilities: [
            "execution_policy.v1",
            "business_inputs.v1",
            "structured_results.v1",
            "artifact_access.v1",
        ]
        .into_iter()
        .chain(
            loaded
                .document
                .role_registry
                .is_some()
                .then_some("native_roles.v1"),
        )
        .collect(),
        binding: loaded.binding,
        actions,
        resources: loaded.document.resources,
        interfaces: loaded.document.interfaces,
        artifact_scopes: loaded.document.artifact_scopes,
        supported_catalog_versions: vec![2, 3],
        supported_request_versions: vec![2, 3],
        role_registry: loaded.document.role_registry,
    })
}
pub(crate) fn parse_request(raw: &str) -> Result<ActionRequest, ActionError> {
    if raw.len() > MAX_REQUEST_BYTES {
        return Err(ActionError::inputs("Request exceeds its byte limit"));
    }
    let value = strict_json(raw.as_bytes()).map_err(ActionError::inputs)?;
    validate_envelope_bounds(&value, 0).map_err(ActionError::inputs)?;
    if let Some(version) = value.get("schema_version").and_then(Value::as_u64) {
        if ![2, 3].contains(&version) {
            return Err(ActionError::new(
                "action.unsupported_contract",
                "Unsupported action request contract version.",
            ));
        }
    }
    let request: ActionRequest = serde_json::from_value(value).map_err(ActionError::inputs)?;
    if (request.schema_version == 3) != request.role_execution.is_some() {
        return Err(ActionError::new(
            "role.unsupported_contract",
            "Private role execution requires action request version 3.",
        ));
    }
    if let Some(role) = &request.role_execution {
        identifier(&role.mode, "Feature mode").map_err(ActionError::inputs)?;
        if let Some(identity) = &role.resolution_id {
            validate_digest(identity).map_err(ActionError::inputs)?;
        }
    }
    identifier(&request.action, "Action ID").map_err(ActionError::inputs)?;
    identifier(&request.interface, "Interface ID").map_err(ActionError::inputs)?;
    validate_value_bounds(&Value::Object(request.inputs.clone()), 0)
        .map_err(ActionError::inputs)?;
    if request.attachment_grants.len() > MAX_MEMBERS {
        return Err(ActionError::inputs("Too many attachment grants"));
    }
    for (id, grant) in &request.attachment_grants {
        identifier(id, "Attachment grant ID").map_err(ActionError::inputs)?;
        validate_digest(&grant.content_sha256).map_err(ActionError::inputs)?;
    }
    Ok(request)
}
pub(crate) fn prepare(root: &Path, request: &ActionRequest) -> Result<PreparedAction, ActionError> {
    if ![2, 3].contains(&request.schema_version) {
        return Err(ActionError::new(
            "action.unsupported_contract",
            "Unsupported action request contract version.",
        ));
    }
    validate_value_bounds(&Value::Object(request.inputs.clone()), 0)
        .map_err(ActionError::inputs)?;
    crate::execution_policy::ExecutionPolicy::parse(&request.execution_policy).map_err(|_| {
        ActionError::new(
            "action.execution_policy_invalid",
            "The action execution policy is invalid.",
        )
    })?;
    let mut loaded = load_catalog(root)?;
    if (request.schema_version == 3) != request.role_execution.is_some()
        || loaded.document.role_registry.is_some() != request.role_execution.is_some()
    {
        return Err(ActionError::new(
            "role.unsupported_contract",
            "This action requires matching native role execution and catalog contracts.",
        ));
    }
    if let (Some(registry), Some(role)) = (&loaded.document.role_registry, &request.role_execution)
    {
        crate::role_contract::validate_bindings(registry, &role.binding_revision)
            .map_err(role_error)?;
        let identity = role.resolution_id.as_ref().ok_or_else(|| {
            ActionError::new(
                "role.resolution_required",
                "Role execution requires the exact reviewed resolution identity.",
            )
        })?;
        validate_digest(identity).map_err(ActionError::inputs)?;
        if !registry.contexts.iter().any(|context| {
            context.key.action == request.action
                && context.key.interface == request.interface
                && context.key.mode == role.mode
        }) {
            return Err(ActionError::new(
                "role.unknown_context",
                "The requested action, interface and feature mode is not declared.",
            ));
        }
    }
    if loaded.binding != request.expected_binding {
        return Err(ActionError::new(
            "action.stale_binding",
            "The action binding changed; rediscover before authorizing this invocation.",
        ));
    }
    let interface = loaded
        .document
        .interfaces
        .iter()
        .find(|interface| interface.id == request.interface)
        .ok_or_else(|| {
            ActionError::new(
                "action.unknown_interface",
                "The selected interface is not declared.",
            )
        })?;
    if !interface.actions.contains(&request.action) {
        return Err(ActionError::new(
            "action.unknown_action",
            "The selected interface does not declare this action.",
        ));
    }
    let action = loaded
        .document
        .actions
        .iter()
        .find(|a| a.id == request.action)
        .ok_or_else(|| {
            ActionError::new(
                "action.unknown_action",
                "The selected action is not declared.",
            )
        })?;
    validate_input_value(
        &action.input_schema,
        &Value::Object(request.inputs.clone()),
        0,
    )
    .map_err(ActionError::inputs)?;
    let definition_bytes = loaded
        .files
        .get(&action.target)
        .ok_or("Missing action target inventory")?;
    let definition_json = String::from_utf8(definition_bytes.clone())
        .map_err(|_| "Action definition must be UTF-8 JSON".to_string())?;
    let definition = crate::runtime_definition::RuntimeAgentDefinition::from_str(&definition_json)?;
    let raw_definition = strict_json(definition_json.as_bytes()).map_err(ActionError::catalog)?;
    let producer_scopes: Vec<String> = serde_json::from_value(
        raw_definition
            .get("result")
            .and_then(|value| value.get("artifact_scopes"))
            .cloned()
            .unwrap_or_else(|| json!([])),
    )
    .map_err(ActionError::catalog)?;
    let scoped_data = request
        .role_execution
        .as_ref()
        .and_then(|execution| {
            loaded.document.role_registry.as_ref().and_then(|registry| {
                registry.contexts.iter().find(|context| {
                    context.key.action == request.action
                        && context.key.interface == request.interface
                        && context.key.mode == execution.mode
                })
            })
        })
        .map(|context| &context.data_scopes)
        .unwrap_or(&interface.artifact_scopes);
    let allowed_scopes = super::action_artifacts::validate_access(
        &loaded.document.artifact_scopes,
        scoped_data,
        &producer_scopes,
        request.artifact_access.as_ref(),
    )
    .map_err(artifact_error)?;
    let artifact_context = if allowed_scopes.is_empty() {
        None
    } else {
        Some(artifact_context(&loaded, interface, allowed_scopes)?)
    };
    let named = definition.named_inputs();
    let mut run_vars = action
        .constants
        .runtime_vars
        .iter()
        .map(|(name, value)| Ok(format!("{name}={}", scalar_text(value)?)))
        .collect::<Result<Vec<_>, String>>()?;
    let mut input_overrides = action
        .constants
        .inputs
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>();
    let mut used_grants = BTreeSet::new();
    for (field, mapping) in &action.mappings {
        let Some(value) = request.inputs.get(field) else {
            continue;
        };
        match mapping {
            InputMapping::RuntimeVar(mapping) => run_vars.push(format!(
                "{}={}",
                mapping.runtime_var,
                mapped_text(value, mapping.encoding.as_deref()).map_err(ActionError::inputs)?
            )),
            InputMapping::Input(mapping) => {
                let kind = named
                    .iter()
                    .find(|input| input.name.as_deref() == Some(mapping.input.as_str()))
                    .ok_or("Action mapping names an undeclared input")?
                    .kind;
                let text = match kind {
                    crate::InputKind::Text => mapped_text(value, mapping.encoding.as_deref())
                        .map_err(ActionError::inputs)?,
                    crate::InputKind::Url => {
                        let url = value.as_str().ok_or_else(|| {
                            ActionError::inputs("URL action input must be a string")
                        })?;
                        if !(url.starts_with("https://") || url.starts_with("http://"))
                            || url.chars().any(char::is_control)
                        {
                            return Err(ActionError::inputs(
                                "URL action inputs require an HTTP(S) URL.",
                            ));
                        }
                        url.to_string()
                    }
                    crate::InputKind::Image | crate::InputKind::File => {
                        let object = value.as_object().ok_or_else(||ActionError::inputs("Attachment action input must name a scoped grant and content_sha256"))?;
                        if object.len() != 2 {
                            return Err(ActionError::inputs(
                                "Attachment input accepts only grant_id and content_sha256.",
                            ));
                        }
                        let id = object
                            .get("grant_id")
                            .and_then(Value::as_str)
                            .ok_or_else(|| ActionError::inputs("Attachment grant ID missing"))?;
                        let expected = object
                            .get("content_sha256")
                            .and_then(Value::as_str)
                            .ok_or_else(|| ActionError::inputs("Attachment digest missing"))?;
                        validate_digest(expected).map_err(ActionError::inputs)?;
                        let grant = request
                            .attachment_grants
                            .get(id)
                            .ok_or_else(|| ActionError::inputs("Attachment grant unavailable"))?;
                        if grant.content_sha256 != expected {
                            return Err(ActionError::inputs(
                                "Attachment grant content identity mismatch.",
                            ));
                        }
                        if !grant.path.is_absolute() {
                            return Err(ActionError::inputs(
                                "Attachment grant path must be absolute.",
                            ));
                        }
                        let parent = grant
                            .path
                            .parent()
                            .ok_or_else(|| ActionError::inputs("Attachment parent"))?;
                        let name = grant
                            .path
                            .file_name()
                            .ok_or_else(|| ActionError::inputs("Attachment name"))?;
                        let grant_path = super::local_packages::resolve_existing_path_under_root(
                            parent,
                            Path::new(name),
                            "Attachment grant",
                        )
                        .map_err(ActionError::inputs)?;
                        let bytes = read_bounded(&grant_path, MAX_ATTACHMENT_BYTES, "Attachment")
                            .map_err(ActionError::inputs)?;
                        if sha256(&bytes) != expected {
                            return Err(ActionError::inputs(
                                "Attachment bytes changed from their supplied content identity.",
                            ));
                        }
                        used_grants.insert(id.to_string());
                        grant
                            .path
                            .to_str()
                            .ok_or_else(|| ActionError::inputs("Attachment path is not UTF-8"))?
                            .to_string()
                    }
                };
                input_overrides.push(format!("{}={text}", mapping.input));
            }
        }
    }
    if request
        .attachment_grants
        .keys()
        .any(|id| !used_grants.contains(id))
    {
        return Err(ActionError::inputs(
            "Action request contains an unused attachment grant.",
        ));
    }
    let supplied_vars = run_vars
        .iter()
        .filter_map(|value| value.split_once('=').map(|(name, _)| name))
        .collect::<BTreeSet<_>>();
    if definition
        .runtime_var_specs()
        .iter()
        .any(|var| var.default_value.is_none() && !supplied_vars.contains(var.name.as_str()))
    {
        return Err(ActionError::inputs(
            "Required runtime variable has no action input or constant",
        ));
    }
    let supplied_inputs = input_overrides
        .iter()
        .filter_map(|value| value.split_once('=').map(|(name, _)| name))
        .collect::<BTreeSet<_>>();
    if named.iter().any(|input| {
        input.value.is_none()
            && input
                .name
                .as_ref()
                .is_some_and(|name| !supplied_inputs.contains(name.as_str()))
    }) {
        return Err(ActionError::inputs(
            "Required named input has no action value",
        ));
    }
    let definition_path = confined_file(&loaded.root, &action.target)?;
    if let Some(installed) = loaded.installed.as_mut() {
        let entry = installed
            .context
            .entrypoints
            .iter()
            .find(|entry| entry.path == action.target && entry.runnable)
            .ok_or("Installed action target is not an exported runnable entrypoint")?;
        installed.context.current_entrypoint_path = Some(entry.path.clone());
    }
    Ok(PreparedAction {
        interface: interface.id.clone(),
        project_root: loaded.root.clone(),
        action: action.id.clone(),
        binding: loaded.binding,
        definition_path,
        definition_json,
        run_vars,
        input_overrides,
        installed: loaded.installed,
        execution_policy: request.execution_policy.clone(),
        artifact_context,
        role_execution: request.role_execution.clone(),
    })
}
pub(crate) fn resource(
    root: &Path,
    interface: &str,
    id: &str,
    expected: &ActionBinding,
) -> Result<Value, ActionError> {
    let loaded = load_catalog(root)?;
    if &loaded.binding != expected {
        return Err(ActionError::new(
            "action.stale_binding",
            "The resource binding changed; rediscovery is required.",
        ));
    }
    let selected = loaded
        .document
        .interfaces
        .iter()
        .find(|candidate| candidate.id == interface)
        .ok_or_else(|| {
            ActionError::new(
                "action.unknown_interface",
                "The selected interface is not declared.",
            )
        })?;
    if !selected.resources.iter().any(|resource| resource == id) {
        return Err(ActionError::new(
            "action.resource_denied",
            "The selected interface does not declare this resource.",
        ));
    }
    let resource = loaded
        .document
        .resources
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| {
            ActionError::new(
                "action.resource_denied",
                "The selected resource is not declared.",
            )
        })?;
    let bytes = loaded
        .files
        .get(&resource.path)
        .ok_or("Missing action resource inventory")?;
    Ok(
        json!({"schema_version":1,"binding":loaded.binding,"interface":interface,"id":resource.id,"mime_type":resource.mime_type,
        "size_bytes":bytes.len(),"content_sha256":sha256(bytes),"encoding":"base64","data":STANDARD.encode(bytes)}),
    )
}

fn load_catalog(root: &Path) -> Result<LoadedCatalog, ActionError> {
    let metadata = fs::symlink_metadata(root).map_err(|e| e.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("Action root must be a real directory.".into());
    }
    let root = fs::canonicalize(root).map_err(|e| format!("Cannot resolve action root: {e}"))?;
    let installed = super::local_packages::checked_runtime_lease_for_path(&root, None)?;
    if let Some(checked) = &installed {
        let payload =
            fs::canonicalize(&checked.context.package_payload_root).map_err(|e| e.to_string())?;
        if payload != root {
            return Err("Installed actions must use the verified package payload root.".into());
        }
    }
    let installed_assets = installed
        .as_ref()
        .map(|checked| super::local_packages::installed_action_assets(&checked.context))
        .transpose()?;
    if installed_assets.as_ref().is_some_and(|assets| {
        !assets
            .iter()
            .any(|asset| selection_contains(asset, CATALOG_FILE))
    }) {
        return Err(ActionError::catalog(
            "Installed catalog is not a selected asset",
        ));
    }
    let catalog = confined_file(&root, CATALOG_FILE)?;
    let bytes = read_bounded(&catalog, MAX_CATALOG_BYTES, "Action catalog")?;
    let value: Value = strict_json(&bytes).map_err(|e| format!("Invalid action catalog: {e}"))?;
    validate_envelope_bounds(&value, 0)?;
    let document: CatalogDocument =
        serde_json::from_value(value).map_err(|e| format!("Invalid action catalog: {e}"))?;
    if ![2, 3].contains(&document.schema_version) {
        return Err(ActionError::new(
            "action.unsupported_contract",
            "Unsupported action catalog contract version.",
        ));
    }
    if (document.schema_version == 3) != document.role_registry.is_some() {
        return Err(ActionError::new(
            "role.unsupported_contract",
            "Role declarations require catalog version 3 and its role registry.",
        ));
    }
    if document.actions.is_empty()
        || document.actions.len() > 256
        || document.resources.len() > MAX_MEMBERS
        || document.interfaces.is_empty()
        || document.interfaces.len() > MAX_MEMBERS
    {
        return Err(
            "Action catalogs require 1–256 actions, 1–64 interfaces and at most 64 resources."
                .into(),
        );
    }
    super::action_artifacts::validate_scopes(&document.artifact_scopes).map_err(artifact_error)?;
    let mut ids = BTreeSet::new();
    let mut files = BTreeMap::new();
    let mut total_bytes = bytes.len();
    for action in &document.actions {
        identifier(&action.id, "Action ID")?;
        if !ids.insert(&action.id) {
            return Err(format!("Duplicate action ID `{}`.", action.id).into());
        }
        for text in [action.label.as_ref(), action.description.as_ref()]
            .into_iter()
            .flatten()
        {
            if text.len() > 4096 {
                return Err("Action display metadata exceeds its 4096-byte limit.".into());
            }
        }
        validate_schema(&action.input_schema, 0)?;
        if action.input_schema.get("type").and_then(Value::as_str) != Some("object") {
            return Err("Action input_schema must be an object schema.".into());
        }
        let path = portable_path(&action.target)?;
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            return Err("Action targets must be explicit JSON definition paths.".into());
        }
        let target_path = confined_file(&root, &action.target)?;
        let target_bytes = read_bounded(&target_path, MAX_DEFINITION_BYTES, "Action target")?;
        let text =
            std::str::from_utf8(&target_bytes).map_err(|_| "Action definition must be UTF-8")?;
        let definition = crate::runtime_definition::RuntimeAgentDefinition::from_str(text)?;
        for capability in &action.required_capabilities {
            if ![
                "execution_policy.v1",
                "business_inputs.v1",
                "structured_results.v1",
                "artifact_access.v1",
                "native_roles.v1",
            ]
            .contains(&capability.as_str())
            {
                return Err(ActionError::new(
                    "action.unsupported_capability",
                    "The action requires an unsupported execution capability.",
                ));
            }
        }
        validate_mappings(action, &definition)?;
        if let Some(checked) = &installed {
            if !checked
                .context
                .entrypoints
                .iter()
                .any(|entry| entry.path == action.target && entry.runnable)
            {
                return Err(format!(
                    "Installed action `{}` targets an unexported entrypoint.",
                    action.id
                )
                .into());
            }
        }
        insert_inventory(&mut files, &mut total_bytes, &action.target, target_bytes)?;
    }
    ids.clear();
    for resource in &document.resources {
        if installed_assets.as_ref().is_some_and(|assets| {
            !assets
                .iter()
                .any(|asset| selection_contains(asset, &resource.path))
        }) {
            return Err(ActionError::catalog(
                "Installed resource is not a selected asset",
            ));
        }
        identifier(&resource.id, "Resource ID")?;
        if !ids.insert(&resource.id) {
            return Err(format!("Duplicate resource ID `{}`.", resource.id).into());
        }
        validate_mime(&resource.mime_type)?;
        let path = confined_file(&root, &resource.path)?;
        insert_inventory(
            &mut files,
            &mut total_bytes,
            &resource.path,
            read_bounded(&path, MAX_RESOURCE_BYTES, "Action resource")?,
        )?;
    }
    ids.clear();
    for interface in &document.interfaces {
        identifier(&interface.id, "Interface ID")?;
        if !ids.insert(&interface.id) {
            return Err("Duplicate interface ID.".into());
        }
        if interface
            .label
            .as_ref()
            .is_some_and(|label| label.len() > 4096)
        {
            return Err("Interface label exceeds 4096 bytes.".into());
        }
        if interface.actions.is_empty()
            || interface.actions.len() > 256
            || interface.resources.len() > 64
        {
            return Err("Interface subsets exceed their limits or contain no action.".into());
        }
        // Check interface declarations without conferring runtime permission.
        super::action_artifacts::validate_access(
            &document.artifact_scopes,
            &interface.artifact_scopes,
            &[],
            None,
        )
        .map_err(artifact_error)?;
        let mut action_ids = BTreeSet::new();
        for id in &interface.actions {
            if !action_ids.insert(id) || !document.actions.iter().any(|action| &action.id == id) {
                return Err("Interface references an undeclared or duplicate action.".into());
            }
        }
        let mut resource_ids = BTreeSet::new();
        for id in &interface.resources {
            if !resource_ids.insert(id)
                || !document.resources.iter().any(|resource| &resource.id == id)
            {
                return Err("Interface references an undeclared or duplicate resource.".into());
            }
        }
        if let Some(presentation) = &interface.presentation {
            if !interface.resources.contains(&presentation.entrypoint)
                || !document.resources.iter().any(|resource| {
                    resource.id == presentation.entrypoint && resource.mime_type == "text/html"
                })
            {
                return Err(
                    "Interface presentation must name a selected text/html resource.".into(),
                );
            }
        }
    }
    if let Some(registry) = &document.role_registry {
        validate_role_inventory(
            &root,
            &document,
            registry,
            &installed,
            &mut files,
            &mut total_bytes,
        )?;
    }
    let inventory: Vec<_> = files
        .iter()
        .map(|(path, bytes)| json!({"path":path,"sha256":sha256(bytes)}))
        .collect();
    let binding = ActionBinding {
        root_sha256: sha256(root.to_string_lossy().as_bytes()),
        catalog_sha256: sha256(&bytes),
        content_sha256: sha256(&serde_json::to_vec(&inventory).map_err(|e| e.to_string())?),
        package: installed.as_ref().map(|checked| PackageBinding {
            alias: checked.context.alias.clone(),
            version: checked.context.package_version.clone(),
            content_sha256: checked.context.content_sha256.clone(),
        }),
    };
    Ok(LoadedCatalog {
        root,
        document,
        binding,
        files,
        installed,
    })
}

fn role_error(error: crate::role_contract::RoleError) -> ActionError {
    ActionError::new(error.code, error.message)
}

/// Private native file evidence. Never serialize this absolute-path inventory into package data.
pub(crate) fn verified_role_inventory(
    root: &Path,
) -> Result<BTreeMap<String, String>, ActionError> {
    let loaded = load_catalog(root)?;
    if loaded.document.role_registry.is_none() {
        return Err(ActionError::new(
            "role.unsupported_contract",
            "Native role inventory requires a role catalog.",
        ));
    }
    let mut inventory = BTreeMap::new();
    for (key, bytes) in &loaded.files {
        let (path, digest) = if let Some(relative) = key.strip_prefix("native-child:") {
            (
                confined_file(&loaded.root, relative)?,
                String::from_utf8(bytes.clone()).map_err(ActionError::catalog)?,
            )
        } else if let Some(tool) = key.strip_prefix("native-tool:") {
            let (name, target) = tool
                .split_once(':')
                .ok_or_else(|| ActionError::catalog("Invalid private tool evidence"))?;
            let runtime_root = if let Some(checked) = &loaded.installed {
                super::local_packages::resolve_package_runtime_tools_root(&checked.context)?
                    .ok_or_else(|| ActionError::catalog("Missing private tool runtime"))?
            } else {
                super::tools::project_tools_root(&loaded.root)
            };
            let manifest_path = confined_file(&runtime_root, &format!("{name}/tool.json"))?;
            let raw = read_bounded(&manifest_path, MAX_DEFINITION_BYTES, "Native tool identity")?;
            if target == "manifest" {
                (manifest_path, sha256(&raw))
            } else {
                let manifest = strict_json(&raw).map_err(ActionError::catalog)?;
                let relative = manifest
                    .get("artifacts")
                    .and_then(|v| v.get(target))
                    .and_then(|v| v.get("path"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| ActionError::catalog("Missing native tool artifact"))?;
                (
                    confined_file(&runtime_root.join(name), relative)?,
                    String::from_utf8(bytes.clone()).map_err(ActionError::catalog)?,
                )
            }
        } else {
            (confined_file(&loaded.root, key)?, sha256(bytes))
        };
        inventory.insert(path.to_string_lossy().into_owned(), digest);
    }
    let catalog = confined_file(&loaded.root, CATALOG_FILE)?;
    inventory.insert(
        catalog.to_string_lossy().into_owned(),
        loaded.binding.catalog_sha256,
    );
    Ok(inventory)
}

/// Resolve one declared action context against native, already validated context evidence.
/// The returned selections are policy inputs; they do not authorize an invocation.
pub(crate) fn resolve_roles_from_context(
    root: &Path,
    expected: &ActionBinding,
    key: &crate::role_contract::ContextKey,
    revision: &crate::role_contract::BindingRevision,
    context: &crate::role_contract::ResolutionContext,
    evidence: &[crate::role_contract::CapabilityEvidence],
    now_unix_secs: u64,
) -> Result<crate::role_contract::Resolution, ActionError> {
    let loaded = load_catalog(root)?;
    if &loaded.binding != expected {
        return Err(ActionError::new(
            "action.stale_binding",
            "The complete package action binding changed; rediscover before review.",
        ));
    }
    let package_identity =
        crate::role_contract::canonical_identity(&loaded.binding).map_err(role_error)?;
    if context.package_identity != package_identity {
        return Err(ActionError::new(
            "role.stale_context",
            "The role resolution package context is stale.",
        ));
    }
    let registry = loaded.document.role_registry.as_ref().ok_or_else(|| {
        ActionError::new(
            "role.unsupported_contract",
            "This catalog does not declare native roles.",
        )
    })?;
    crate::role_contract::resolve(registry, key, revision, context, evidence, now_unix_secs)
        .map_err(role_error)
}

fn validate_role_inventory(
    root: &Path,
    document: &CatalogDocument,
    registry: &crate::role_contract::RoleRegistry,
    installed: &Option<super::local_packages::CheckedInstalledPackageRuntime>,
    files: &mut BTreeMap<String, Vec<u8>>,
    total_bytes: &mut usize,
) -> Result<(), ActionError> {
    use crate::role_contract::{CallKind, CallLocator};
    crate::role_contract::validate_registry(registry).map_err(role_error)?;
    let mut definitions = BTreeSet::new();
    for action in &document.actions {
        definitions.insert(action.target.clone());
    }
    for site in &registry.call_sites {
        definitions.insert(site.locator.definition.clone());
        if let Some(target) = &site.target {
            definitions.insert(target.clone());
        }
        if let Some(schema) = &site.input_schema {
            validate_schema(schema, 0)?;
        }
    }
    if definitions.len() > crate::role_contract::MAX_CALL_SITES {
        return Err(ActionError::catalog("Too many role definitions"));
    }
    let mut actual = BTreeMap::new();
    let mut tools = BTreeSet::new();
    for path in &definitions {
        if let Some(checked) = installed {
            if !checked
                .context
                .entrypoints
                .iter()
                .any(|entry| entry.path == *path && entry.runnable)
            {
                return Err(ActionError::catalog(
                    "Role definition must be an exported native entrypoint",
                ));
            }
        }
        let bytes = read_bounded(
            &confined_file(root, path)?,
            MAX_DEFINITION_BYTES,
            "Role definition",
        )?;
        let text = std::str::from_utf8(&bytes).map_err(ActionError::catalog)?;
        crate::runtime_definition::RuntimeAgentDefinition::from_str(text)?;
        let definition = strict_json(&bytes).map_err(ActionError::catalog)?;
        if definition
            .pointer("/agent_schema/properties")
            .and_then(Value::as_object)
            .is_some_and(|properties| !properties.is_empty())
        {
            actual.insert(
                CallLocator {
                    definition: path.clone(),
                    site: "root".into(),
                },
                (CallKind::Root, definition.clone(), None),
            );
        }
        for (action_index, action) in definition["actions"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            for (step_index, step) in action["run"].as_array().into_iter().flatten().enumerate() {
                let kind = match step["kind"].as_str() {
                    Some("generate_image") => CallKind::Image,
                    Some("generate_audio") => CallKind::Audio,
                    Some("transcribe_audio") => CallKind::Transcription,
                    Some("agent") => CallKind::Child,
                    Some("tool") => {
                        if let Some(name) = step["name"].as_str() {
                            tools.insert((path.clone(), name.to_string()));
                        }
                        continue;
                    }
                    _ => continue,
                };
                let target = if kind == CallKind::Child {
                    let target = step
                        .get("artifact")
                        .or_else(|| step.get("agent"))
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            ActionError::catalog("Role child target must be finite and declarative")
                        })?;
                    let target = target.strip_prefix("./").unwrap_or(target);
                    let relative = Path::new(path)
                        .parent()
                        .unwrap_or(Path::new(""))
                        .join(target);
                    let target = relative
                        .to_str()
                        .ok_or_else(|| ActionError::catalog("Invalid child target"))?
                        .replace('\\', "/");
                    if crate::role_contract::portable_definition(&target)
                        && definitions.contains(&target)
                    {
                        Some(target)
                    } else {
                        let declared = registry.call_sites.iter().find(|site| site.locator.definition == *path && site.locator.site == format!("actions.{action_index}.run.{step_index}") && site.artifact.as_ref() == Some(&target)).ok_or_else(|| ActionError::catalog("Generated child requires an exact artifact and source declaration"))?;
                        let artifact_path = confined_file(root, &target)?;
                        let capability = crate::generated_capabilities::capabilities_for_artifact(
                            &artifact_path,
                        )
                        .map_err(|_| {
                            ActionError::catalog("Generated native role child requires rebuilding")
                        })?;
                        if !capability.supports_native_roles() {
                            return Err(ActionError::catalog(
                                "Generated native role child does not support this contract",
                            ));
                        }
                        let source = declared.target.as_ref().ok_or_else(|| {
                            ActionError::catalog("Generated child source is missing")
                        })?;
                        let source_value = strict_json(&read_bounded(
                            &confined_file(root, source)?,
                            MAX_DEFINITION_BYTES,
                            "Generated child source",
                        )?)
                        .map_err(ActionError::catalog)?;
                        let source_identity =
                            crate::role_contract::canonical_identity(&source_value)
                                .map_err(role_error)?;
                        if crate::generated_capabilities::definition_for_artifact(&artifact_path)
                            .ok()
                            .as_deref()
                            != Some(&source_identity)
                        {
                            return Err(ActionError::catalog(
                                "Generated child source changed; rebuild the declared artifact",
                            ));
                        }
                        let artifact_bytes = read_bounded(
                            &artifact_path,
                            128 * 1024 * 1024,
                            "Generated native child",
                        )?;
                        insert_inventory(
                            files,
                            total_bytes,
                            &format!("native-child:{target}"),
                            sha256(&artifact_bytes).into_bytes(),
                        )?;
                        Some(source.clone())
                    }
                } else {
                    None
                };
                actual.insert(
                    CallLocator {
                        definition: path.clone(),
                        site: format!("actions.{action_index}.run.{step_index}"),
                    },
                    (kind, step.clone(), target),
                );
            }
        }
        insert_inventory(files, total_bytes, path, bytes)?;
    }
    for site in &registry.call_sites {
        if site.kind == CallKind::ToolChild {
            let parts: Vec<_> = site.locator.site.split('.').collect();
            if parts.len() != 3
                || !tools.contains(&(site.locator.definition.clone(), parts[1].to_string()))
            {
                return Err(ActionError::catalog(
                    "Tool call site must name a tool used by its declaring definition",
                ));
            }
            let target = site.target.as_ref().unwrap();
            let definition = strict_json(
                files
                    .get(target)
                    .ok_or_else(|| ActionError::catalog("Missing native tool child definition"))?,
            )
            .map_err(ActionError::catalog)?;
            if site
                .input_schema
                .as_ref()
                .and_then(|v| v.get("properties"))
                .and_then(Value::as_object)
                .is_none_or(|props| {
                    props.keys().any(|key| {
                        definition
                            .get("runtime_vars")
                            .and_then(|v| v.get(key))
                            .is_none()
                    })
                })
            {
                return Err(ActionError::catalog(
                    "Tool child business keys must name declared child runtime variables",
                ));
            }
            if let Some(artifact) = &site.artifact {
                let path = confined_file(root, artifact)?;
                let capabilities = crate::generated_capabilities::capabilities_for_artifact(&path)
                    .map_err(|_| ActionError::catalog("Native tool child requires rebuilding"))?;
                let source =
                    crate::role_contract::canonical_identity(&definition).map_err(role_error)?;
                if !capabilities.supports_native_roles()
                    || capabilities.is_cli_run()
                    || crate::generated_capabilities::definition_for_artifact(&path)
                        .ok()
                        .as_deref()
                        != Some(&source)
                {
                    return Err(ActionError::catalog(
                        "Native tool child source or contract requires rebuilding",
                    ));
                }
                let bytes = read_bounded(&path, 128 * 1024 * 1024, "Native tool child")?;
                insert_inventory(
                    files,
                    total_bytes,
                    &format!("native-child:{artifact}"),
                    sha256(&bytes).into_bytes(),
                )?;
            }
            continue;
        }
        let (kind, selectors, target) = actual.remove(&site.locator).ok_or_else(|| {
            ActionError::catalog("Role call site is not an actual native location")
        })?;
        if kind != site.kind || target != site.target {
            return Err(ActionError::catalog(
                "Role call kind or target does not match its native definition",
            ));
        }
        for selector in ["profile", "model", "thinking"] {
            if let Some(value) = selectors.get(selector) {
                if site.role.is_some() {
                    return Err(ActionError::catalog(
                        "Mapped call sites cannot silently replace explicit native selectors",
                    ));
                }
                let fixed = site.fixed.as_ref().ok_or_else(|| {
                    ActionError::catalog(
                        "Structural child declarations cannot hide explicit native selectors",
                    )
                })?;
                let expected = match selector {
                    "profile" => serde_json::to_value(&fixed.profile).unwrap_or(Value::Null),
                    "model" => match &fixed.model {
                        crate::execution_policy::ModelSelection::Named { value } => {
                            Value::String(value.clone())
                        }
                        _ => Value::Null,
                    },
                    _ => fixed
                        .settings
                        .get("thinking")
                        .cloned()
                        .unwrap_or_else(|| json!({"mode":"provider_default"})),
                };
                if value != &expected {
                    return Err(ActionError::catalog(
                        "Fixed call declarations must disclose exact native selectors",
                    ));
                }
            }
        }
    }
    if !actual.is_empty() {
        return Err(ActionError::catalog(
            "Every native root, media and direct child call must be declared",
        ));
    }
    let tool_names: BTreeSet<_> = tools.iter().map(|(_, name)| name).collect();
    if registry
        .tool_content
        .keys()
        .any(|name| !tool_names.contains(name))
    {
        return Err(ActionError::catalog(
            "Role tool inventory contains an unrelated tool",
        ));
    }
    for name in tool_names {
        let content = registry.tool_content.get(name).ok_or_else(|| {
            ActionError::catalog("Native role tools require an explicit portable source inventory")
        })?;
        if content.is_empty()
            || content.len() > 256
            || content.iter().collect::<BTreeSet<_>>().len() != content.len()
        {
            return Err(ActionError::catalog(
                "Role tool source inventory is empty, duplicated or oversized",
            ));
        }
        let manifest_relative = format!(".cargo-ai/tools/{name}/tool.json");
        let manifest_bytes = read_bounded(
            &confined_file(root, &manifest_relative)?,
            MAX_DEFINITION_BYTES,
            "Role tool manifest",
        )?;
        let manifest = strict_json(&manifest_bytes).map_err(ActionError::catalog)?;
        let source_manifest = manifest
            .pointer("/source/manifest_path")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ActionError::catalog("Role tools require portable source-backed manifests")
            })?;
        if !content.iter().any(|path| path == source_manifest) {
            return Err(ActionError::catalog(
                "Role tool source inventory omits its source manifest",
            ));
        }
        let source_root = Path::new(source_manifest)
            .parent()
            .ok_or_else(|| ActionError::catalog("Role tool source needs a containing directory"))?;
        if source_root.as_os_str().is_empty() {
            return Err(ActionError::catalog(
                "Role tool source must be in its own directory",
            ));
        }
        let mut source_files = Vec::new();
        collect_role_source_files(root, source_root, &mut source_files, 0)?;
        if source_files.iter().any(|path| !content.contains(path))
            || content.iter().any(|path| !source_files.contains(path))
        {
            return Err(ActionError::catalog(
                "Role tool inventory must contain the complete portable source tree",
            ));
        }
        insert_inventory(files, total_bytes, &manifest_relative, manifest_bytes)?;
        for path in content {
            insert_inventory(
                files,
                total_bytes,
                path,
                read_bounded(
                    &confined_file(root, path)?,
                    MAX_DEFINITION_BYTES,
                    "Role tool source",
                )?,
            )?;
        }
        // Runtime binaries are host-bound evidence; they are never added to portable content.
        let runtime_root = if let Some(checked) = installed {
            super::local_packages::resolve_package_runtime_tools_root(&checked.context)?
                .ok_or_else(|| ActionError::catalog("Installed role tool runtime is unavailable"))?
        } else {
            super::tools::project_tools_root(root)
        };
        let runtime_manifest_path = confined_file(&runtime_root, &format!("{name}/tool.json"))?;
        let runtime_manifest = strict_json(&read_bounded(
            &runtime_manifest_path,
            MAX_DEFINITION_BYTES,
            "Native role tool manifest",
        )?)
        .map_err(ActionError::catalog)?;
        insert_inventory(
            files,
            total_bytes,
            &format!("native-tool:{name}:manifest"),
            serde_json::to_vec(&runtime_manifest).map_err(ActionError::catalog)?,
        )?;
        let artifacts = runtime_manifest
            .get("artifacts")
            .and_then(Value::as_object)
            .ok_or_else(|| ActionError::catalog("Native role tool artifacts are unavailable"))?;
        if artifacts.len() > 16 {
            return Err(ActionError::catalog("Too many native role tool artifacts"));
        }
        for (target, artifact) in artifacts {
            let relative = artifact
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| ActionError::catalog("Invalid native role tool artifact"))?;
            let base = runtime_root.join(name);
            let binary_path = confined_file(&base, relative)?;
            let binary =
                read_bounded(&binary_path, 128 * 1024 * 1024, "Native role tool artifact")?;
            insert_inventory(
                files,
                total_bytes,
                &format!("native-tool:{name}:{target}"),
                sha256(&binary).into_bytes(),
            )?;
        }
    }
    for context in &registry.contexts {
        let action = document
            .actions
            .iter()
            .find(|a| a.id == context.key.action)
            .ok_or_else(|| ActionError::catalog("Unknown role action dependency"))?;
        let interface = document
            .interfaces
            .iter()
            .find(|i| i.id == context.key.interface && i.actions.contains(&action.id))
            .ok_or_else(|| ActionError::catalog("Unknown role interface dependency"))?;
        if context
            .resources
            .iter()
            .any(|id| !interface.resources.contains(id))
            || context
                .data_scopes
                .iter()
                .any(|id| !interface.artifact_scopes.contains(id))
            || (registry.call_sites.iter().any(|site| {
                site.locator.definition == action.target && site.kind == CallKind::Root
            }) && !context.call_sites.iter().any(|id| {
                registry.call_sites.iter().any(|site| {
                    &site.id == id
                        && site.locator.definition == action.target
                        && site.kind == CallKind::Root
                })
            }))
        {
            return Err(ActionError::catalog(
                "Role action context must retain its native root and declared resource/data scopes",
            ));
        }
    }
    for interface in &document.interfaces {
        for action in &interface.actions {
            if !registry
                .contexts
                .iter()
                .any(|c| c.key.action == *action && c.key.interface == interface.id)
            {
                return Err(ActionError::catalog(
                    "Every action and interface requires an explicit feature-mode declaration",
                ));
            }
        }
    }
    Ok(())
}

fn collect_role_source_files(
    root: &Path,
    relative: &Path,
    files: &mut Vec<String>,
    depth: usize,
) -> Result<(), ActionError> {
    if depth > 16 || files.len() > 256 {
        return Err(ActionError::catalog(
            "Role tool source inventory exceeds its finite bounds",
        ));
    }
    let directory = super::local_packages::resolve_existing_path_under_root(
        root,
        relative,
        "Role tool source",
    )?;
    for entry in fs::read_dir(directory).map_err(ActionError::catalog)? {
        let entry = entry.map_err(ActionError::catalog)?;
        let name = entry.file_name();
        if matches!(name.to_str(), Some("target" | ".git")) {
            continue;
        }
        let path = relative.join(name);
        let kind = entry.file_type().map_err(ActionError::catalog)?;
        if kind.is_symlink() {
            return Err(ActionError::catalog(
                "Role tool sources cannot contain links",
            ));
        }
        if kind.is_dir() {
            collect_role_source_files(root, &path, files, depth + 1)?;
        } else if kind.is_file() {
            files.push(
                path.to_str()
                    .ok_or_else(|| ActionError::catalog("Role tool source path is not portable"))?
                    .replace('\\', "/"),
            );
        } else {
            return Err(ActionError::catalog(
                "Role tool source contains an unsupported file",
            ));
        }
    }
    Ok(())
}

// Reject duplicates and excessive structure before converting catalogs/requests to Value.
fn strict_json(bytes: &[u8]) -> Result<Value, String> {
    crate::business_schema::strict_json_bounded(bytes, MAX_DEFINITION_BYTES, 64)
}

fn insert_inventory(
    files: &mut BTreeMap<String, Vec<u8>>,
    total: &mut usize,
    path: &str,
    bytes: Vec<u8>,
) -> Result<(), String> {
    if let Some(previous) = files.get(path) {
        if previous != &bytes {
            return Err("Action inventory changed during discovery.".into());
        }
    } else {
        *total += bytes.len();
        if *total > MAX_INVENTORY_BYTES {
            return Err("Action inventory exceeds the 16 MiB limit.".into());
        }
        files.insert(path.to_string(), bytes);
    }
    Ok(())
}
fn portable_path(raw: &str) -> Result<PathBuf, String> {
    let path = super::runtime_data::validate_declared_input(raw)?;
    let portable = path
        .to_str()
        .ok_or("Action path must be UTF-8")?
        .replace('\\', "/");
    if portable != raw
        || raw.len() > 1024
        || raw
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("Action paths must use canonical portable relative paths.".into());
    }
    Ok(path)
}
fn confined_file(root: &Path, raw: &str) -> Result<PathBuf, String> {
    let relative = portable_path(raw)?;
    let path =
        super::local_packages::resolve_existing_path_under_root(root, &relative, "Action asset")?;
    if !path.is_file() {
        return Err("Action assets must be regular files.".into());
    }
    Ok(path)
}
fn read_bounded(path: &Path, limit: usize, label: &str) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|e| format!("Cannot read {label}: {e}"))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(format!("{label} must be a regular file no larger than {limit} bytes.").into());
    }
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err(format!("{label} exceeds its {limit}-byte limit.").into());
    }
    Ok(bytes)
}
fn identifier(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value.as_bytes()[0].is_ascii_alphabetic()
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        || matches!(
            value,
            "execution_policy"
                | "expected_binding"
                | "attachment_grants"
                | "__proto__"
                | "constructor"
                | "prototype"
        )
    {
        return Err(
            format!("{label} must be a non-reserved ASCII identifier of 1–128 bytes.").into(),
        );
    }
    Ok(())
}
fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn validate_digest(value: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    {
        return Err("content_sha256 must be a lowercase SHA-256 hex digest.".into());
    }
    Ok(())
}
fn validate_mime(mime: &str) -> Result<(), String> {
    if !matches!(
        mime,
        "text/html"
            | "text/css"
            | "text/javascript"
            | "text/plain"
            | "application/json"
            | "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "image/svg+xml"
    ) {
        return Err(format!("Unsupported action resource MIME type `{mime}`.").into());
    }
    Ok(())
}
fn validate_envelope_bounds(value: &Value, depth: usize) -> Result<(), String> {
    if depth > 32 {
        return Err("Action document exceeds its nesting limit.".into());
    }
    match value {
        Value::Object(values) => {
            if values.len() > 1024 {
                return Err("Action document has too many fields.".into());
            }
            for value in values.values() {
                validate_envelope_bounds(value, depth + 1)?;
            }
        }
        Value::Array(values) => {
            if values.len() > 1024 {
                return Err("Action document has too many items.".into());
            }
            for value in values {
                validate_envelope_bounds(value, depth + 1)?;
            }
        }
        Value::String(text) if text.len() > 64 * 1024 => {
            return Err("Action document string exceeds 64 KiB.".into())
        }
        _ => {}
    }
    Ok(())
}
fn validate_value_bounds(value: &Value, depth: usize) -> Result<(), String> {
    crate::business_schema::validate_value_bounds(value, depth)
}
fn validate_schema(schema: &Value, depth: usize) -> Result<(), String> {
    crate::business_schema::validate_schema(schema, depth)
}
fn validate_input_value(schema: &Value, value: &Value, depth: usize) -> Result<(), String> {
    crate::business_schema::validate_value(schema, value, depth)
}
fn mapped_text(value: &Value, encoding: Option<&str>) -> Result<String, String> {
    match encoding {
        Some("json") => Ok(value.to_string()),
        None => scalar_text(value),
        Some(_) => Err("Unsupported action mapping encoding.".into()),
    }
}
fn scalar_text(value: &Value) -> Result<String, String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Bool(_) | Value::Number(_) => Ok(value.to_string()),
        _ => Err("Runtime variable action mappings require scalar inputs.".into()),
    }
}
fn validate_mappings(
    action: &ActionDocument,
    definition: &crate::runtime_definition::RuntimeAgentDefinition,
) -> Result<(), String> {
    let properties = action
        .input_schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or("Missing action properties")?;
    if action.mappings.len() != properties.len()
        || action
            .mappings
            .keys()
            .any(|name| !properties.contains_key(name))
    {
        return Err("Every action business input must have exactly one declared mapping.".into());
    }
    let vars = definition.runtime_var_specs();
    let inputs = definition.named_inputs();
    let mut destinations = BTreeSet::new();
    for (name, mapping) in &action.mappings {
        let kind = properties[name]
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("nullable");
        match mapping {
            InputMapping::RuntimeVar(mapping) => {
                identifier(&mapping.runtime_var, "Runtime variable mapping")?;
                if !destinations.insert(format!("var:{}", mapping.runtime_var)) {
                    return Err("Duplicate action mapping destination.".into());
                }
                let var = vars
                    .iter()
                    .find(|var| var.name == mapping.runtime_var)
                    .ok_or("Action maps an undeclared runtime variable")?;
                let expected = match var.field_type {
                    crate::RuntimeVarType::String => "string",
                    crate::RuntimeVarType::Boolean => "boolean",
                    crate::RuntimeVarType::Integer => "integer",
                    crate::RuntimeVarType::Number => "number",
                };
                if let Some(encoding) = mapping.encoding.as_deref() {
                    if encoding != "json" || expected != "string" {
                        return Err(
                            "JSON action mappings require a string runtime variable.".into()
                        );
                    }
                } else if kind != expected && !(expected == "number" && kind == "integer") {
                    return Err(
                        "Action runtime variable mapping type does not match its declaration."
                            .into(),
                    );
                }
            }
            InputMapping::Input(mapping) => {
                identifier(&mapping.input, "Named input mapping")?;
                if !destinations.insert(format!("input:{}", mapping.input)) {
                    return Err("Duplicate action mapping destination.".into());
                }
                let input = inputs
                    .iter()
                    .find(|input| input.name.as_deref() == Some(mapping.input.as_str()))
                    .ok_or("Action maps an undeclared named input")?;
                if let Some(encoding) = mapping.encoding.as_deref() {
                    if encoding != "json" || input.kind != crate::InputKind::Text {
                        return Err("JSON action input mappings require a text input.".into());
                    }
                }
                if input.kind == crate::InputKind::Text
                    && matches!(kind, "array" | "object" | "null" | "nullable")
                    && mapping.encoding.is_none()
                {
                    return Err(
                        "Structured action input mappings require explicit JSON encoding.".into(),
                    );
                }
                match input.kind {
                    crate::InputKind::Text => {}
                    crate::InputKind::Url if kind == "string" => {}
                    crate::InputKind::File | crate::InputKind::Image if kind == "object" => {
                        let schema = &properties[name];
                        let fields = schema
                            .get("properties")
                            .and_then(Value::as_object)
                            .ok_or("Attachment requires properties")?;
                        let required = schema
                            .get("required")
                            .and_then(Value::as_array)
                            .ok_or("Attachment requires required fields")?;
                        if fields.len() != 2
                            || fields
                                .get("grant_id")
                                .and_then(|v| v.get("type"))
                                .and_then(Value::as_str)
                                != Some("string")
                            || fields
                                .get("content_sha256")
                                .and_then(|v| v.get("type"))
                                .and_then(Value::as_str)
                                != Some("string")
                            || required.len() != 2
                            || !required.contains(&json!("grant_id"))
                            || !required.contains(&json!("content_sha256"))
                        {
                            return Err("Attachment input schema must require exactly grant_id and content_sha256 strings.".into());
                        }
                    }
                    _ => {
                        return Err(
                            "Action named input mapping type does not match its declaration."
                                .into(),
                        )
                    }
                }
            }
        }
    }
    for capability in &action.required_capabilities {
        if ![
            "execution_policy.v1",
            "business_inputs.v1",
            "structured_results.v1",
            "artifact_access.v1",
        ]
        .contains(&capability.as_str())
        {
            return Err(format!("Unsupported required action capability `{capability}`.").into());
        }
    }
    let mut required_caps = BTreeSet::new();
    if action
        .required_capabilities
        .iter()
        .any(|cap| !required_caps.insert(cap))
    {
        return Err("Duplicate required action capability.".into());
    }
    for (name, value) in &action.constants.runtime_vars {
        identifier(name, "Constant runtime variable")?;
        if !destinations.insert(format!("var:{name}")) {
            return Err("Action constant/input mapping collision.".into());
        }
        let var = vars
            .iter()
            .find(|var| &var.name == name)
            .ok_or("Action constant names an undeclared runtime variable")?;
        let valid = match var.field_type {
            crate::RuntimeVarType::String => value.is_string(),
            crate::RuntimeVarType::Boolean => value.is_boolean(),
            crate::RuntimeVarType::Integer => value.as_i64().is_some() || value.as_u64().is_some(),
            crate::RuntimeVarType::Number => value.is_number(),
        };
        if !valid {
            return Err("Action constant type does not match its runtime variable.".into());
        }
    }
    for name in action.constants.inputs.keys() {
        identifier(name, "Constant named input")?;
        if !destinations.insert(format!("input:{name}")) {
            return Err("Action constant/input mapping collision.".into());
        }
        let input = inputs
            .iter()
            .find(|input| input.name.as_deref() == Some(name.as_str()))
            .ok_or("Action constant names an undeclared input")?;
        if input.kind != crate::InputKind::Text {
            return Err("Action input constants support text inputs only.".into());
        }
    }
    Ok(())
}

/// Validate semantics before replacing a build or installed payload. Assets and
/// targets are selected explicitly by the build profile, never by the catalog.
pub(crate) fn validate_distribution(
    root: &Path,
    targets: &[String],
    assets: &[String],
) -> Result<(), String> {
    let selected = assets
        .iter()
        .any(|asset| selection_contains(asset, CATALOG_FILE));
    if !selected {
        // An authored catalog that is not selected is intentionally omitted.
        return Ok(());
    }
    let loaded = load_catalog(root).map_err(|error| error.to_string())?;
    for action in &loaded.document.actions {
        if !targets.iter().any(|target| target == &action.target) {
            return Err(format!(
                "Action `{}` target `{}` is not explicitly selected by this profile.",
                action.id, action.target
            )
            .into());
        }
    }
    if let Some(registry) = &loaded.document.role_registry {
        for site in &registry.call_sites {
            for path in std::iter::once(&site.locator.definition).chain(site.target.as_ref()) {
                if !targets.contains(path) {
                    return Err("Every role call-site definition and native target must be explicitly selected by this profile.".into());
                }
            }
        }
    }
    for resource in &loaded.document.resources {
        if !assets
            .iter()
            .any(|asset| selection_contains(asset, &resource.path))
        {
            return Err(format!(
                "Action resource `{}` is not explicitly selected by this profile.",
                resource.path
            )
            .into());
        }
    }
    Ok(())
}
pub(crate) fn validate_installed_distribution(
    root: &Path,
    targets: &[String],
    assets: &[String],
) -> Result<(), String> {
    if !assets
        .iter()
        .any(|asset| selection_contains(asset, CATALOG_FILE))
    {
        return Err("Present action catalog is missing from the package asset selection.".into());
    }
    validate_distribution(root, targets, assets)
}
fn selection_contains(selection: &str, path: &str) -> bool {
    if selection == path {
        return true;
    }
    let prefix = selection.trim_end_matches('/');
    !prefix.is_empty()
        && path
            .strip_prefix(prefix)
            .is_some_and(|remaining| remaining.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        root: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("cargo-ai-actions-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            let root = fs::canonicalize(root).unwrap();
            fs::write(
                root.join("agent.json"),
                include_str!("../../templates/guidance/examples/client-action-coordinator.json"),
            )
            .unwrap();
            fs::write(root.join("page.html"), "<script src='page.js'></script>").unwrap();
            fs::write(root.join("page.js"), "initial").unwrap();
            let fixture = Self { root };
            fixture.write_catalog(&Self::catalog());
            fixture
        }
        fn catalog() -> Value {
            json!({"schema_version":2,"actions":[{"id":"generate","target":"agent.json","input_schema":{"type":"object","properties":{"panel_ids":{"type":"array","items":{"type":"string","minLength":1},"minItems":1,"maxItems":16}},"required":["panel_ids"],"additionalProperties":false},"mappings":{"panel_ids":{"runtime_var":"panel_ids_json","encoding":"json"}},"required_capabilities":["execution_policy.v1"]}],"interfaces":[{"id":"panels","actions":["generate"],"resources":["page","script"],"presentation":{"entrypoint":"page"}},{"id":"native","actions":["generate"]}],"resources":[{"id":"page","path":"page.html","mime_type":"text/html"},{"id":"script","path":"page.js","mime_type":"text/javascript"}]})
        }
        fn write_catalog(&self, value: &Value) {
            fs::write(
                self.root.join(CATALOG_FILE),
                serde_json::to_vec(value).unwrap(),
            )
            .unwrap();
        }
        fn request(&self) -> ActionRequest {
            ActionRequest {
                schema_version: 2,
                interface: "panels".into(),
                action: "generate".into(),
                inputs: serde_json::from_value(json!({"panel_ids":["p1","p2"]})).unwrap(),
                expected_binding: discover(&self.root).unwrap().binding,
                execution_policy: json!({"version":1,"allowed":[],"limits":{"max_runtime_secs":60,"max_output_tokens":512,"max_agent_depth":4}}),
                attachment_grants: BTreeMap::new(),
                artifact_access: None,
                role_execution: None,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn role_fixture() -> (Fixture, Value) {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("agent.json"), serde_json::to_vec(&json!({"agent_definition_schema_version":"2026-10-03.r1","agent_schema":{"type":"object","properties":{"answer":{"type":"string"}}},"actions":[]})).unwrap()).unwrap();
        let mut catalog = Fixture::catalog();
        catalog["schema_version"] = json!(3);
        catalog["actions"][0]["input_schema"] =
            json!({"type":"object","properties":{},"required":[],"additionalProperties":false});
        catalog["actions"][0]["mappings"] = json!({});
        let requirements = json!({"operation":"text_generation","input_modalities":["text"],"structured_output":false,"settings":{}});
        catalog["role_registry"] = json!({"version":1,"roles":[{"id":"writer","label":"Writer","purpose":"Write content","requirements":requirements}],"call_sites":[{"id":"root","locator":{"definition":"agent.json","site":"root"},"kind":"root","role":"writer","requirements":requirements}],"contexts":[{"key":{"action":"generate","interface":"panels","mode":"default"},"call_sites":["root"],"resources":["page"],"limits":{"max_runtime_secs":60,"max_output_tokens":512,"max_agent_depth":4}},{"key":{"action":"generate","interface":"native","mode":"default"},"call_sites":["root"],"resources":[],"limits":{"max_runtime_secs":60,"max_output_tokens":512,"max_agent_depth":4}}]});
        fixture.write_catalog(&catalog);
        crate::runtime_definition::RuntimeAgentDefinition::from_str(
            &fs::read_to_string(fixture.root.join("agent.json")).unwrap(),
        )
        .unwrap();
        crate::role_contract::validate_registry(
            &serde_json::from_value(catalog["role_registry"].clone()).unwrap(),
        )
        .unwrap();
        (fixture, catalog)
    }

    #[test]
    fn role_contract_shared_role_does_not_grant_another_interfaces_resources() {
        let (fixture, mut catalog) = role_fixture();
        let mut other_action = catalog["actions"][0].clone();
        other_action["id"] = json!("narrate");
        catalog["actions"]
            .as_array_mut()
            .unwrap()
            .push(other_action);
        catalog["interfaces"][0]["resources"] = json!(["page"]);
        catalog["interfaces"][1]["actions"] = json!(["narrate"]);
        catalog["interfaces"][1]["resources"] = json!(["script"]);
        catalog["role_registry"]["contexts"][1]["key"]["action"] = json!("narrate");
        catalog["role_registry"]["contexts"][1]["resources"] = json!(["script"]);
        fixture.write_catalog(&catalog);
        let discovered = discover(&fixture.root).unwrap();
        assert_eq!(discovered.role_registry.unwrap().roles.len(), 1);
        assert!(resource(&fixture.root, "panels", "page", &discovered.binding).is_ok());
        assert!(resource(&fixture.root, "native", "script", &discovered.binding).is_ok());
        assert_eq!(
            resource(&fixture.root, "panels", "script", &discovered.binding)
                .unwrap_err()
                .code,
            "action.resource_denied"
        );
        assert_eq!(
            resource(&fixture.root, "native", "page", &discovered.binding)
                .unwrap_err()
                .code,
            "action.resource_denied"
        );
    }

    #[test]
    fn role_contract_discovery_is_package_wide_and_legacy_requests_cannot_ignore_roles() {
        let (fixture, _) = role_fixture();
        let discovery = discover(&fixture.root).unwrap();
        let registry = discovery.role_registry.unwrap();
        assert_eq!(registry.roles.len(), 1);
        assert_eq!(registry.contexts.len(), 2);
        assert!(discovery.capabilities.contains(&"native_roles.v1"));
        let mut request = fixture.request();
        request.inputs.clear();
        assert_eq!(
            prepare(&fixture.root, &request).unwrap_err().code,
            "role.unsupported_contract"
        );
        request.schema_version = 3;
        request.role_execution = Some(RoleExecution {
            binding_revision: crate::role_contract::BindingRevision {
                version: 1,
                revision: "draft".into(),
                bindings: vec![],
            },
            resolution_id: None,
            mode: "default".into(),
        });
        assert_eq!(
            prepare(&fixture.root, &request).unwrap_err().code,
            "role.resolution_required"
        );
    }

    #[test]
    fn role_contract_catalog_rejects_omitted_native_sites_and_wrong_feature_dependencies() {
        let (fixture, mut catalog) = role_fixture();
        catalog["role_registry"]["call_sites"][0]["locator"]["site"] = json!("actions.0.run.0");
        fixture.write_catalog(&catalog);
        assert!(discover(&fixture.root).is_err());
        catalog["role_registry"]["call_sites"][0]["locator"]["site"] = json!("root");
        catalog["role_registry"]["contexts"][1]["resources"] = json!(["page"]);
        fixture.write_catalog(&catalog);
        assert!(discover(&fixture.root).is_err());
        catalog["role_registry"]["contexts"][1]["resources"] = json!([]);
        catalog["schema_version"] = json!(2);
        fixture.write_catalog(&catalog);
        assert_eq!(
            discover(&fixture.root).unwrap_err().code,
            "role.unsupported_contract"
        );
    }

    #[test]
    fn role_contract_source_identity_invalidates_whole_package_review() {
        let (fixture, _) = role_fixture();
        let before = discover(&fixture.root).unwrap().binding;
        fs::write(fixture.root.join("page.js"), "changed-resource").unwrap();
        let after = discover(&fixture.root).unwrap().binding;
        assert_ne!(before, after);
    }

    #[test]
    fn role_contract_structural_children_require_no_fabricated_root_role() {
        let (fixture, mut catalog) = role_fixture();
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
        catalog["role_registry"]["roles"] = json!([]);
        catalog["role_registry"]["call_sites"] = json!([{"id":"launch","locator":{"definition":"agent.json","site":"actions.0.run.0"},"kind":"child","target":"child.json","requirements":{"operation":"text_generation"}}]);
        for context in catalog["role_registry"]["contexts"].as_array_mut().unwrap() {
            context["call_sites"] = json!(["launch"]);
        }
        fixture.write_catalog(&catalog);
        let registry = discover(&fixture.root).unwrap().role_registry.unwrap();
        assert!(registry.roles.is_empty());
        assert_eq!(registry.call_sites.len(), 1);
        assert!(registry.call_sites[0].role.is_none());
        assert!(registry.call_sites[0].fixed.is_none());
    }

    #[test]
    fn role_contract_tool_manifest_source_and_native_artifact_drift_change_binding() {
        let (fixture, mut catalog) = role_fixture();
        let mut definition: Value =
            serde_json::from_slice(&fs::read(fixture.root.join("agent.json")).unwrap()).unwrap();
        fs::write(
            fixture.root.join("child.json"),
            serde_json::to_vec(&definition).unwrap(),
        )
        .unwrap();
        definition["actions"] =
            json!([{"name":"call","logic":{"==":[1,1]},"run":[{"kind":"tool","name":"helper"}]}]);
        fs::write(
            fixture.root.join("agent.json"),
            serde_json::to_vec(&definition).unwrap(),
        )
        .unwrap();
        let root = catalog["role_registry"]["call_sites"][0].clone();
        let mut child = root.clone();
        child["id"] = json!("child-root");
        child["locator"]["definition"] = json!("child.json");
        let mut tool = root;
        tool["id"] = json!("tool-child");
        tool["kind"] = json!("tool_child");
        tool["locator"]["site"] = json!("tools.helper.child");
        tool["target"] = json!("child.json");
        tool["input_schema"] =
            json!({"type":"object","properties":{},"required":[],"additionalProperties":false});
        catalog["role_registry"]["call_sites"]
            .as_array_mut()
            .unwrap()
            .extend([child, tool]);
        for context in catalog["role_registry"]["contexts"].as_array_mut().unwrap() {
            context["call_sites"] = json!(["root", "tool-child", "child-root"]);
        }
        catalog["role_registry"]["tool_content"] =
            json!({"helper":["tools/helper/Cargo.toml","tools/helper/src/main.rs"]});
        fs::create_dir_all(fixture.root.join("tools/helper/src")).unwrap();
        fs::create_dir_all(fixture.root.join(".cargo-ai/tools/helper/bin")).unwrap();
        fs::write(
            fixture.root.join("tools/helper/Cargo.toml"),
            "[package]\nname='helper'\nversion='0.1.0'\n",
        )
        .unwrap();
        fs::write(
            fixture.root.join("tools/helper/src/main.rs"),
            "fn main() {}\n",
        )
        .unwrap();
        fs::write(fixture.root.join(".cargo-ai/tools/helper/tool.json"),serde_json::to_vec(&json!({"schema_version":1,"tool_id":"helper","source":{"manifest_path":"tools/helper/Cargo.toml"},"artifacts":{"test-target":{"path":"bin/helper"}}})).unwrap()).unwrap();
        fs::write(
            fixture.root.join(".cargo-ai/tools/helper/bin/helper"),
            "native-fixture-one",
        )
        .unwrap();
        fixture.write_catalog(&catalog);
        let before = discover(&fixture.root).unwrap().binding;
        fs::write(
            fixture.root.join(".cargo-ai/tools/helper/bin/helper"),
            "native-fixture-two",
        )
        .unwrap();
        let binary_change = discover(&fixture.root).unwrap().binding;
        assert_ne!(before, binary_change);
        fs::write(
            fixture.root.join("tools/helper/src/main.rs"),
            "fn main() { panic!() }\n",
        )
        .unwrap();
        assert_ne!(binary_change, discover(&fixture.root).unwrap().binding);
        fs::write(fixture.root.join("tools/helper/src/unlisted.rs"), "hidden").unwrap();
        assert!(discover(&fixture.root).is_err());
    }

    #[test]
    fn action_resolution_encodes_structured_inputs_and_executes_loaded_bytes_only() {
        let fixture = Fixture::new();
        let request = fixture.request();
        let prepared = prepare(&fixture.root, &request).unwrap();
        assert_eq!(prepared.run_vars, vec!["panel_ids_json=[\"p1\",\"p2\"]"]);
        assert_eq!(
            prepared.definition_json,
            fs::read_to_string(fixture.root.join("agent.json")).unwrap()
        );
        assert!(prepared.installed.is_none());
    }
    #[test]
    fn action_binding_rejects_script_only_target_catalog_and_root_changes() {
        let fixture = Fixture::new();
        let request = fixture.request();
        fs::write(fixture.root.join("page.js"), "changed").unwrap();
        assert_eq!(
            prepare(&fixture.root, &request).unwrap_err().code,
            "action.stale_binding"
        );
        assert_eq!(
            resource(&fixture.root, "panels", "page", &request.expected_binding)
                .unwrap_err()
                .code,
            "action.stale_binding"
        );
        fs::write(fixture.root.join("page.js"), "initial").unwrap();
        fs::write(
            fixture.root.join("agent.json"),
            format!(
                "{}\n",
                fs::read_to_string(fixture.root.join("agent.json")).unwrap()
            ),
        )
        .unwrap();
        assert_eq!(
            prepare(&fixture.root, &request).unwrap_err().code,
            "action.stale_binding"
        );
        let other = Fixture::new();
        assert_eq!(
            prepare(&other.root, &request).unwrap_err().code,
            "action.stale_binding"
        );
    }
    #[test]
    fn action_resources_are_bounded_scoped_and_digest_bound() {
        let fixture = Fixture::new();
        let binding = discover(&fixture.root).unwrap().binding;
        let value = resource(&fixture.root, "panels", "script", &binding).unwrap();
        assert_eq!(value["content_sha256"], sha256(b"initial"));
        assert_eq!(
            STANDARD.decode(value["data"].as_str().unwrap()).unwrap(),
            b"initial"
        );
        assert_eq!(
            resource(&fixture.root, "native", "script", &binding)
                .unwrap_err()
                .code,
            "action.resource_denied"
        );
        assert_eq!(
            resource(&fixture.root, "missing", "script", &binding)
                .unwrap_err()
                .code,
            "action.unknown_interface"
        );
        fs::write(
            fixture.root.join("page.js"),
            vec![0; MAX_RESOURCE_BYTES + 1],
        )
        .unwrap();
        assert!(discover(&fixture.root).is_err());
    }
    #[test]
    fn action_catalog_rejects_versions_unknown_schema_fields_and_unsafe_mappings() {
        let fixture = Fixture::new();
        let base = Fixture::catalog();
        let mut candidate = base.clone();
        candidate["schema_version"] = json!(1);
        fixture.write_catalog(&candidate);
        assert_eq!(
            discover(&fixture.root).unwrap_err().code,
            "action.unsupported_contract"
        );
        for pointer in [
            "/surprise",
            "/actions/0/surprise",
            "/actions/0/input_schema/$ref",
            "/actions/0/mappings/panel_ids/surprise",
        ] {
            let mut candidate = base.clone();
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            let object = if parent.is_empty() {
                candidate.as_object_mut().unwrap()
            } else {
                candidate
                    .pointer_mut(parent)
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
            };
            object.insert(key.into(), json!("bad"));
            fixture.write_catalog(&candidate);
            assert!(discover(&fixture.root).is_err(), "{pointer}");
        }
        let mut candidate = base.clone();
        candidate["actions"][0]["mappings"]["panel_ids"]
            .as_object_mut()
            .unwrap()
            .remove("encoding");
        fixture.write_catalog(&candidate);
        assert!(discover(&fixture.root).is_err());
        let mut candidate = base.clone();
        candidate["actions"][0]["constants"] =
            json!({"runtime_vars":{"panel_ids_json":"collision"}});
        fixture.write_catalog(&candidate);
        assert!(discover(&fixture.root).is_err());
        let mut candidate = base;
        candidate["actions"][0]["required_capabilities"] = json!(["unknown.v1"]);
        fixture.write_catalog(&candidate);
        assert_eq!(
            discover(&fixture.root).unwrap_err().code,
            "action.unsupported_capability"
        );
    }
    #[test]
    fn action_inputs_reject_unknown_wrong_types_depth_and_policy() {
        let fixture = Fixture::new();
        let mut request = fixture.request();
        request.inputs.insert("profile".into(), json!("untrusted"));
        assert_eq!(
            prepare(&fixture.root, &request).unwrap_err().code,
            "action.invalid_inputs"
        );
        request.inputs.remove("profile");
        request.inputs.insert("panel_ids".into(), json!([3]));
        assert_eq!(
            prepare(&fixture.root, &request).unwrap_err().code,
            "action.invalid_inputs"
        );
        let mut request = fixture.request();
        request.execution_policy = json!({});
        assert_eq!(
            prepare(&fixture.root, &request).unwrap_err().code,
            "action.execution_policy_invalid"
        );
        let mut nested = json!(true);
        for _ in 0..10 {
            nested = json!([nested]);
        }
        assert!(validate_value_bounds(&nested, 0).is_err());
        assert!(parse_request(&" ".repeat(MAX_REQUEST_BYTES + 1)).is_err());
    }
    #[test]
    fn action_json_rejects_duplicate_keys_and_preserves_integer_bounds() {
        assert!(strict_json(br#"{"id":"first","id":"second"}"#).is_err());
        assert!(strict_json(br#"{"outer":{"value":1,"value":2}}"#).is_err());
        let schema = json!({"type":"integer","maximum":9007199254740992u64});
        assert!(validate_input_value(&schema, &json!(9007199254740993u64), 0).is_err());
        assert!(validate_input_value(&schema, &json!(9007199254740992u64), 0).is_ok());
        assert_eq!(
            parse_request(r#"{"schema_version":1}"#).unwrap_err().code,
            "action.unsupported_contract"
        );
    }
    #[test]
    fn action_attachment_grants_require_declared_scopes_and_matching_bytes() {
        let fixture = Fixture::new();
        let mut catalog = Fixture::catalog();
        fs::write(fixture.root.join("agent.json"),r#"{"agent_definition_schema_version":"2026-10-01.r1","agent_schema":{"type":"object","properties":{"answer":{"type":"string"}}},"inputs":[{"name":"document","type":"file","path":"default.pdf"}],"actions":[]}"#).unwrap();
        catalog["actions"][0]["input_schema"] = json!({"type":"object","properties":{"document":{"type":"object","properties":{"grant_id":{"type":"string"},"content_sha256":{"type":"string"}},"required":["grant_id","content_sha256"],"additionalProperties":false}},"required":["document"],"additionalProperties":false});
        catalog["actions"][0]["mappings"] = json!({"document":{"input":"document"}});
        let definition = crate::runtime_definition::RuntimeAgentDefinition::from_str(
            &fs::read_to_string(fixture.root.join("agent.json")).unwrap(),
        )
        .unwrap();
        let action: ActionDocument = serde_json::from_value(catalog["actions"][0].clone()).unwrap();
        validate_schema(&action.input_schema, 0).unwrap();
        validate_mappings(&action, &definition).unwrap();
        fixture.write_catalog(&catalog);
        let mut request = fixture.request();
        request.inputs = serde_json::from_value(
            json!({"document":{"grant_id":"selected","content_sha256":sha256(b"content")}}),
        )
        .unwrap();
        assert_eq!(
            prepare(&fixture.root, &request).unwrap_err().code,
            "action.invalid_inputs"
        );
        fs::write(fixture.root.join("attachment.pdf"), "content").unwrap();
        request.attachment_grants.insert(
            "selected".into(),
            AttachmentGrant {
                path: fixture.root.join("attachment.pdf"),
                content_sha256: sha256(b"content"),
            },
        );
        let prepared = prepare(&fixture.root, &request).unwrap();
        assert!(prepared.input_overrides[0].starts_with("document="));
        fs::write(fixture.root.join("attachment.pdf"), "edited").unwrap();
        assert_eq!(
            prepare(&fixture.root, &request).unwrap_err().code,
            "action.invalid_inputs"
        );
    }
    #[test]
    fn action_distribution_requires_explicit_targets_and_assets() {
        let fixture = Fixture::new();
        let targets = vec!["agent.json".into()];
        let assets = vec![CATALOG_FILE.into(), "page.html".into(), "page.js".into()];
        validate_distribution(&fixture.root, &targets, &assets).unwrap();
        assert!(validate_distribution(&fixture.root, &[], &assets).is_err());
        assert!(validate_distribution(&fixture.root, &targets, &assets[..2]).is_err());
        validate_distribution(&fixture.root, &[], &[]).unwrap();
        assert!(validate_installed_distribution(&fixture.root, &targets, &[]).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn action_resources_reject_links_traversal_and_reserved_data() {
        let fixture = Fixture::new();
        let outside = Fixture::new();
        fs::remove_file(fixture.root.join("page.js")).unwrap();
        std::os::unix::fs::symlink(outside.root.join("page.js"), fixture.root.join("page.js"))
            .unwrap();
        assert!(discover(&fixture.root).is_err());
        for path in [
            "../escape",
            "/absolute",
            "C:/escape",
            ".cargo-ai/data/state.json",
            "a\\b",
            "a/./b",
        ] {
            assert!(portable_path(path).is_err(), "{path}");
        }
    }
}
