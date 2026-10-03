//! Passive declared actions and immutable resources for application callers.
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
    fn new(code: &'static str, message: &'static str) -> Self {
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
    pub(crate) actions: Vec<ActionDocument>,
    pub(crate) resources: Vec<ResourceDocument>,
    pub(crate) interfaces: Vec<InterfaceDocument>,
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
        "validate" => {
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
            let prepared = prepare(&target.root, &request).map_err(action_failure)?;
            Ok(
                json!({"schema_version":1,"valid":true,"execution_authorized":false,"interface":prepared.interface,"action":prepared.action,"binding":prepared.binding,
                "mappings":{"runtime_vars":prepared.run_vars.iter().filter_map(|value|value.split_once('=').map(|(name,_)|name)).collect::<Vec<_>>(),"inputs":prepared.input_overrides.iter().filter_map(|value|value.split_once('=').map(|(name,_)|name)).collect::<Vec<_>>()}}),
            )
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
    json!({"catalog_bytes":MAX_CATALOG_BYTES,"actions":256,"interfaces":64,"resources":64,"request_bytes":MAX_REQUEST_BYTES,"business_depth":MAX_DEPTH,"array_items":1024,"object_members":MAX_MEMBERS,"resource_bytes":MAX_RESOURCE_BYTES,"inventory_bytes":MAX_INVENTORY_BYTES,"definition_bytes":MAX_DEFINITION_BYTES,"attachment_bytes":MAX_ATTACHMENT_BYTES,"string_bytes":64*1024})
}
pub(crate) fn discover(root: &Path) -> Result<ActionCatalog, ActionError> {
    let loaded = load_catalog(root)?;
    Ok(ActionCatalog {
        schema_version: 1,
        limits: limits(),
        capabilities: vec!["execution_policy.v1"],
        binding: loaded.binding,
        actions: loaded.document.actions,
        resources: loaded.document.resources,
        interfaces: loaded.document.interfaces,
    })
}
pub(crate) fn parse_request(raw: &str) -> Result<ActionRequest, ActionError> {
    if raw.len() > MAX_REQUEST_BYTES {
        return Err(ActionError::inputs("Request exceeds its byte limit"));
    }
    let value = strict_json(raw.as_bytes()).map_err(ActionError::inputs)?;
    validate_envelope_bounds(&value, 0).map_err(ActionError::inputs)?;
    if let Some(version) = value.get("schema_version").and_then(Value::as_u64) {
        if version != 1 {
            return Err(ActionError::new(
                "action.unsupported_contract",
                "Unsupported action request contract version.",
            ));
        }
    }
    let request: ActionRequest = serde_json::from_value(value).map_err(ActionError::inputs)?;
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
    if request.schema_version != 1 {
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
    if document.schema_version != 1 {
        return Err(ActionError::new(
            "action.unsupported_contract",
            "Unsupported action catalog contract version.",
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
            if capability != "execution_policy.v1" {
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

// Retain duplicate-key rejection before converting catalogs/requests to Value;
// ordinary JSON maps would otherwise silently replace earlier declarations.
fn strict_json(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    struct UniqueValue(Value);
    impl<'de> Deserialize<'de> for UniqueValue {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = UniqueValue;
                fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str("JSON with unique object keys")
                }
                fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                    Ok(UniqueValue(Value::Bool(value)))
                }
                fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                    Ok(UniqueValue(value.into()))
                }
                fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                    Ok(UniqueValue(value.into()))
                }
                fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
                    serde_json::Number::from_f64(value)
                        .map(|n| UniqueValue(Value::Number(n)))
                        .ok_or_else(|| E::custom("Nonfinite number"))
                }
                fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                    Ok(UniqueValue(value.into()))
                }
                fn visit_string<E: serde::de::Error>(
                    self,
                    value: String,
                ) -> Result<Self::Value, E> {
                    Ok(UniqueValue(value.into()))
                }
                fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                    Ok(UniqueValue(Value::Null))
                }
                fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                    Ok(UniqueValue(Value::Null))
                }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut sequence: A,
                ) -> Result<Self::Value, A::Error> {
                    let mut values = Vec::new();
                    while let Some(UniqueValue(value)) = sequence.next_element()? {
                        values.push(value);
                    }
                    Ok(UniqueValue(Value::Array(values)))
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> Result<Self::Value, A::Error> {
                    let mut values = Map::new();
                    while let Some((key, UniqueValue(value))) =
                        map.next_entry::<String, UniqueValue>()?
                    {
                        if values.insert(key, value).is_some() {
                            return Err(serde::de::Error::custom("Duplicate JSON object key"));
                        }
                    }
                    Ok(UniqueValue(Value::Object(values)))
                }
            }
            deserializer.deserialize_any(Visitor)
        }
    }
    serde_json::from_slice::<UniqueValue>(bytes).map(|UniqueValue(value)| value)
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
    if depth > MAX_DEPTH {
        return Err("Action JSON exceeds the eight-level nesting limit.".into());
    }
    match value {
        Value::Object(values) => {
            if values.len() > MAX_MEMBERS {
                return Err("Action JSON object exceeds its 64-member limit.".into());
            }
            for (key, value) in values {
                if key.len() > 1024 {
                    return Err("Action JSON key exceeds its length limit.".into());
                }
                validate_value_bounds(value, depth + 1)?;
            }
        }
        Value::Array(values) => {
            if values.len() > 1024 {
                return Err("Action JSON array exceeds its 1024-item limit.".into());
            }
            for value in values {
                validate_value_bounds(value, depth + 1)?;
            }
        }
        Value::String(text) if text.len() > 64 * 1024 => {
            return Err("Action JSON string exceeds its 64 KiB limit.".into())
        }
        _ => {}
    }
    Ok(())
}
fn validate_schema(schema: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("Action schema nesting exceeds its limit.".into());
    }
    let object = schema
        .as_object()
        .ok_or("Action input schemas must be objects")?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or("Action input schemas require one explicit type")?;
    let allowed: &[&str] = match kind {
        "object" => &[
            "type",
            "properties",
            "required",
            "additionalProperties",
            "description",
        ],
        "array" => &["type", "items", "minItems", "maxItems", "description"],
        "string" => &["type", "minLength", "maxLength", "enum", "description"],
        "integer" | "number" => &["type", "minimum", "maximum", "enum", "description"],
        "boolean" => &["type", "enum", "description"],
        _ => return Err(format!("Unsupported action input schema type `{kind}`.")),
    };
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("Unsupported action input schema keyword `{key}`.").into());
        }
    }
    if object.get("description").is_some_and(|v| !v.is_string()) {
        return Err("Schema description must be a string.".into());
    }
    match kind {
        "object" => {
            if object.get("additionalProperties") != Some(&Value::Bool(false)) {
                return Err("Action object schemas require additionalProperties:false.".into());
            }
            let properties = object
                .get("properties")
                .and_then(Value::as_object)
                .ok_or("Action object schemas require properties")?;
            if properties.len() > MAX_MEMBERS {
                return Err("Too many action input properties.".into());
            }
            for (name, schema) in properties {
                identifier(name, "Action input name")?;
                if matches!(
                    name.as_str(),
                    "profile"
                        | "model"
                        | "thinking"
                        | "server"
                        | "token"
                        | "settings"
                        | "config"
                        | "execution_limits"
                ) {
                    return Err(
                        "Action business schemas must not declare reserved execution fields."
                            .into(),
                    );
                }
                validate_schema(schema, depth + 1)?;
            }
            let mut seen = BTreeSet::new();
            if let Some(required) = object.get("required") {
                for value in required
                    .as_array()
                    .ok_or("Schema required must be an array")?
                {
                    let name = value
                        .as_str()
                        .ok_or("Schema required names must be strings")?;
                    if !properties.contains_key(name) || !seen.insert(name) {
                        return Err(
                            "Schema required contains an undeclared or duplicate input.".into()
                        );
                    }
                }
            }
        }
        "array" => {
            validate_schema(
                object.get("items").ok_or("Action arrays require items")?,
                depth + 1,
            )?;
            validate_range(object, "minItems", "maxItems", 1024)?;
        }
        "string" => {
            validate_range(object, "minLength", "maxLength", 64 * 1024)?;
        }
        "integer" | "number" => {
            for key in ["minimum", "maximum"] {
                if object.get(key).is_some_and(|v| !v.is_number()) {
                    return Err(format!("Schema {key} must be a number.").into());
                }
            }
            if let (Some(min), Some(max)) = (
                object.get("minimum").and_then(Value::as_f64),
                object.get("maximum").and_then(Value::as_f64),
            ) {
                if min > max {
                    return Err("Schema minimum exceeds maximum.".into());
                }
            }
        }
        _ => {}
    }
    if let Some(values) = object.get("enum") {
        let values = values.as_array().ok_or("Schema enum must be an array")?;
        if values.is_empty() || values.len() > 128 {
            return Err("Schema enum requires 1–128 values.".into());
        }
        let mut base = object.clone();
        base.remove("enum");
        for (index, value) in values.iter().enumerate() {
            validate_input_value(&Value::Object(base.clone()), value, depth)?;
            if values[..index].contains(value) {
                return Err("Schema enum contains duplicate values.".into());
            }
        }
    }
    Ok(())
}
fn validate_range(
    object: &Map<String, Value>,
    min_key: &str,
    max_key: &str,
    limit: u64,
) -> Result<(), String> {
    let mut min = 0;
    let mut max = limit;
    for (key, slot) in [(min_key, &mut min), (max_key, &mut max)] {
        if let Some(value) = object.get(key) {
            *slot = value.as_u64().filter(|n| *n <= limit).ok_or_else(|| {
                format!("Schema {key} must be an unsigned integer no larger than {limit}")
            })?;
        }
    }
    if min > max {
        return Err(format!("Schema {min_key} exceeds {max_key}.").into());
    }
    Ok(())
}
fn validate_input_value(schema: &Value, value: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("Action input nesting exceeds its limit.".into());
    }
    if schema
        .get("enum")
        .and_then(Value::as_array)
        .is_some_and(|values| !values.contains(value))
    {
        return Err("Action input is outside its declared enum.".into());
    }
    match schema
        .get("type")
        .and_then(Value::as_str)
        .ok_or("Missing schema type")?
    {
        "object" => {
            let object = value.as_object().ok_or("Action input requires an object")?;
            let properties = schema
                .get("properties")
                .and_then(Value::as_object)
                .ok_or("Missing schema properties")?;
            for name in schema
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if !object.contains_key(name.as_str().ok_or("Invalid required input")?) {
                    return Err(format!("Required action input `{name}` is missing.").into());
                }
            }
            for (name, value) in object {
                validate_input_value(
                    properties
                        .get(name)
                        .ok_or_else(|| format!("Undeclared action input `{name}`."))?,
                    value,
                    depth + 1,
                )?;
            }
        }
        "array" => {
            let values = value.as_array().ok_or("Action input requires an array")?;
            check_length(schema, values.len(), "minItems", "maxItems", 1024)?;
            for value in values {
                validate_input_value(
                    schema.get("items").ok_or("Missing array items")?,
                    value,
                    depth + 1,
                )?;
            }
        }
        "string" => check_length(
            schema,
            value
                .as_str()
                .ok_or("Action input requires a string")?
                .chars()
                .count(),
            "minLength",
            "maxLength",
            64 * 1024,
        )?,
        "boolean" => {
            if !value.is_boolean() {
                return Err("Action input requires a Boolean.".into());
            }
        }
        kind @ ("integer" | "number") => {
            if !value.is_number()
                || (kind == "integer" && value.as_i64().is_none() && value.as_u64().is_none())
            {
                return Err(format!("Action input requires {kind}.").into());
            }
            if schema
                .get("minimum")
                .is_some_and(|bound| numeric_cmp(value, bound) == Some(std::cmp::Ordering::Less))
                || schema.get("maximum").is_some_and(|bound| {
                    numeric_cmp(value, bound) == Some(std::cmp::Ordering::Greater)
                })
            {
                return Err("Action input is outside its declared numeric range.".into());
            }
        }
        _ => return Err("Unsupported action input schema type.".into()),
    }
    Ok(())
}
fn numeric_cmp(left: &Value, right: &Value) -> Option<std::cmp::Ordering> {
    let integer = |value: &Value| {
        value
            .as_i64()
            .map(i128::from)
            .or_else(|| value.as_u64().map(i128::from))
    };
    match (integer(left), integer(right)) {
        (Some(left), Some(right)) => Some(left.cmp(&right)),
        _ => left.as_f64()?.partial_cmp(&right.as_f64()?),
    }
}
fn check_length(
    schema: &Value,
    length: usize,
    min: &str,
    max: &str,
    limit: usize,
) -> Result<(), String> {
    if length < schema.get(min).and_then(Value::as_u64).unwrap_or(0) as usize
        || length
            > schema
                .get(max)
                .and_then(Value::as_u64)
                .unwrap_or(limit as u64) as usize
    {
        return Err("Action input exceeds its declared length/count bounds.".into());
    }
    Ok(())
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
            .ok_or("Missing input type")?;
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
                    if encoding != "json"
                        || expected != "string"
                        || !matches!(kind, "array" | "object")
                    {
                        return Err("JSON action mappings require a structured value and string runtime variable.".into());
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
                    if encoding != "json"
                        || input.kind != crate::InputKind::Text
                        || !matches!(kind, "array" | "object")
                    {
                        return Err(
                            "JSON action input mappings require a structured value and text input."
                                .into(),
                        );
                    }
                }
                if input.kind == crate::InputKind::Text
                    && matches!(kind, "array" | "object")
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
        if capability != "execution_policy.v1" {
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
            json!({"schema_version":1,"actions":[{"id":"generate","target":"agent.json","input_schema":{"type":"object","properties":{"panel_ids":{"type":"array","items":{"type":"string","minLength":1},"minItems":1,"maxItems":16}},"required":["panel_ids"],"additionalProperties":false},"mappings":{"panel_ids":{"runtime_var":"panel_ids_json","encoding":"json"}},"required_capabilities":["execution_policy.v1"]}],"interfaces":[{"id":"panels","actions":["generate"],"resources":["page","script"],"presentation":{"entrypoint":"page"}},{"id":"native","actions":["generate"]}],"resources":[{"id":"page","path":"page.html","mime_type":"text/html"},{"id":"script","path":"page.js","mime_type":"text/javascript"}]})
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
                schema_version: 1,
                interface: "panels".into(),
                action: "generate".into(),
                inputs: serde_json::from_value(json!({"panel_ids":["p1","p2"]})).unwrap(),
                expected_binding: discover(&self.root).unwrap().binding,
                execution_policy: json!({"version":1,"allowed":[],"limits":{"max_runtime_secs":60,"max_output_tokens":512,"max_agent_depth":4}}),
                attachment_grants: BTreeMap::new(),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
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
        candidate["schema_version"] = json!(2);
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
            parse_request(r#"{"schema_version":2}"#).unwrap_err().code,
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
