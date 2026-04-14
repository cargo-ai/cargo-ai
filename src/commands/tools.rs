//! Tool lifecycle and runtime support for Cargo AI-managed companion binaries.
use clap::ArgMatches;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const PROJECT_METADATA_RELATIVE_PATH: &str = ".cargo-ai/project.toml";
const PROJECT_TOOLS_RELATIVE_PATH: &str = ".cargo-ai/tools";
const TOOL_MANIFEST_FILE_NAME: &str = "tool.json";
const TOOL_SCAFFOLD_CARGO_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/tool-scaffold/Cargo.toml.tmpl"
));
const TOOL_SCAFFOLD_MAIN_RS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/tool-scaffold/src/main.rs.tmpl"
));
const TOOL_SCAFFOLD_LIB_RS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/tool-scaffold/src/lib.rs.tmpl"
));
const TOOL_PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ToolScope {
    Bundled,
    Project,
    Machine,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ToolManifestBinary {
    #[serde(default)]
    default_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ToolManifestSource {
    manifest_path: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ToolManifestDistribution {
    #[allow(dead_code)]
    channel: Option<String>,
    #[allow(dead_code)]
    package_id: Option<String>,
    #[allow(dead_code)]
    package_version: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ToolManifestArtifact {
    path: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ToolManifest {
    schema_version: u32,
    tool_id: String,
    #[serde(default)]
    source: Option<ToolManifestSource>,
    #[serde(default)]
    binary: ToolManifestBinary,
    #[serde(default)]
    artifacts: BTreeMap<String, ToolManifestArtifact>,
    #[allow(dead_code)]
    #[serde(default)]
    distribution: Option<ToolManifestDistribution>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ToolDescribeParam {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) required: bool,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) default: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ToolDescribeResult {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) nullable: bool,
    #[serde(default)]
    pub(crate) description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ToolDescribeResourceProfile {
    pub(crate) network: String,
    pub(crate) filesystem_read: String,
    pub(crate) filesystem_write: String,
    pub(crate) subprocess: String,
    pub(crate) env_read: String,
    pub(crate) credential_access: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ToolDescribeSelfTest {
    pub(crate) supported: bool,
    pub(crate) safe: bool,
    #[serde(default)]
    pub(crate) description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ToolDescribeExamples {
    pub(crate) minimal_invoke: Value,
    pub(crate) full_invoke: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ToolDescribeDocument {
    pub(crate) protocol_version: u32,
    pub(crate) name: String,
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) params: BTreeMap<String, ToolDescribeParam>,
    pub(crate) result: ToolDescribeResult,
    pub(crate) resource_profile: ToolDescribeResourceProfile,
    pub(crate) self_test: ToolDescribeSelfTest,
    pub(crate) examples: ToolDescribeExamples,
}

#[derive(Clone, Debug, Deserialize)]
struct ToolInvokeResponse {
    protocol_version: u32,
    result: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedTool {
    pub(crate) tool_id: String,
    pub(crate) scope: ToolScope,
    pub(crate) tool_dir: PathBuf,
    #[allow(dead_code)]
    manifest_path: PathBuf,
    pub(crate) binary_name: String,
    pub(crate) target_triple: String,
    pub(crate) binary_path: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct ToolContract {
    pub(crate) resolved: ResolvedTool,
    pub(crate) describe: ToolDescribeDocument,
}

#[derive(Clone, Debug)]
pub(crate) struct ToolResolver {
    bundled_root: Option<PathBuf>,
    project_root: Option<PathBuf>,
    target_triple: String,
}

impl ToolResolver {
    pub(crate) fn new(project_root: Option<PathBuf>, target_triple: impl Into<String>) -> Self {
        Self {
            bundled_root: None,
            project_root,
            target_triple: target_triple.into(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn with_bundled_root(mut self, bundled_root: Option<PathBuf>) -> Self {
        self.bundled_root = bundled_root;
        self
    }

    pub(crate) fn resolve_contract(&self, tool_id: &str) -> Result<ToolContract, String> {
        let resolved = self.resolve_tool(tool_id)?;
        let describe = load_tool_describe_document(&resolved)?;
        validate_describe_document(&describe, &resolved)?;
        Ok(ToolContract { resolved, describe })
    }

    pub(crate) fn resolve_tool(&self, tool_id: &str) -> Result<ResolvedTool, String> {
        validate_tool_identifier(tool_id)?;

        if let Some(bundled_root) = self.bundled_root.as_ref() {
            if let Some(resolved) = resolve_tool_from_scope_root(
                bundled_root,
                ToolScope::Bundled,
                tool_id,
                &self.target_triple,
            )? {
                return Ok(resolved);
            }
        }

        if let Some(project_root) = self.project_root.as_ref() {
            let project_tools_root = project_tools_root(project_root);
            match resolve_tool_from_scope_root(
                &project_tools_root,
                ToolScope::Project,
                tool_id,
                &self.target_triple,
            ) {
                Ok(Some(resolved)) => return Ok(resolved),
                Ok(None) => {}
                Err(error) => return Err(error),
            }
        }

        let machine_tools_root = machine_tools_root();
        resolve_tool_from_scope_root(
            &machine_tools_root,
            ToolScope::Machine,
            tool_id,
            &self.target_triple,
        )?
        .ok_or_else(|| {
            format!(
                "Tool '{}' was not found in the current project or Cargo AI Home.",
                tool_id
            )
        })
    }
}

pub(crate) fn maybe_find_project_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };

    loop {
        if current.join(PROJECT_METADATA_RELATIVE_PATH).exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

#[allow(dead_code)]
pub(crate) fn bundled_tools_root_from_executable() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .map(|path| path.join(PROJECT_TOOLS_RELATIVE_PATH))
        .filter(|path| path.exists())
}

pub(crate) fn audit_actions_for_tools(
    actions: &[crate::Action],
    resolver: &ToolResolver,
    current_platform: Option<&str>,
) -> Result<Vec<ResolvedTool>, String> {
    let mut contracts = BTreeMap::<String, ToolContract>::new();

    for (action_index, action) in actions.iter().enumerate() {
        for (step_index, step) in action.run.iter().enumerate() {
            if !step_matches_platform(step.platforms.as_deref(), current_platform) {
                continue;
            }
            if !step.kind.eq_ignore_ascii_case("tool") {
                continue;
            }

            let tool_name = step.tool_name.as_deref().ok_or_else(|| {
                format!(
                    "Action '{}' tool step {} is missing required `name`.",
                    action.name,
                    step_index + 1
                )
            })?;
            let contract = if let Some(existing) = contracts.get(tool_name) {
                existing.clone()
            } else {
                let resolved = resolver.resolve_contract(tool_name)?;
                contracts.insert(tool_name.to_string(), resolved.clone());
                resolved
            };

            validate_tool_step_against_contract(
                step,
                &contract.describe,
                Some(action.name.as_str()),
                Some(step_index),
            )?;
            if contract.describe.name != tool_name
                && contract.describe.name != contract.resolved.binary_name
            {
                return Err(format!(
                    "Tool '{}' describe contract reported name '{}', which does not match the referenced tool id.",
                    tool_name, contract.describe.name
                ));
            }
            if action_index == usize::MAX {
                unreachable!();
            }
        }
    }

    Ok(contracts
        .into_values()
        .map(|contract| contract.resolved)
        .collect())
}

pub(crate) fn validate_tool_step_against_contract(
    step: &crate::RunStep,
    describe: &ToolDescribeDocument,
    action_name: Option<&str>,
    step_index: Option<usize>,
) -> Result<(), String> {
    let context = tool_step_context(action_name, step_index);
    let provided = &step.tool_params;

    for (name, value) in provided {
        let Some(param_spec) = describe.params.get(name) else {
            return Err(format!(
                "{context} references unknown tool param '{}'.",
                name
            ));
        };

        if let crate::ToolParamValue::Literal(literal) = value {
            if !json_value_matches_declared_type(literal, param_spec.kind.as_str()) {
                return Err(format!(
                    "{context} param '{}' must be {}, but the literal value is incompatible.",
                    name,
                    display_type_name(param_spec.kind.as_str())
                ));
            }
        }
    }

    for (name, param_spec) in &describe.params {
        if param_spec.required && param_spec.default.is_none() && !provided.contains_key(name) {
            return Err(format!(
                "{context} is missing required tool param '{}'.",
                name
            ));
        }
    }

    Ok(())
}

pub(crate) fn resolve_tool_invoke_params(
    step: &crate::RunStep,
    data: &Value,
    action_name: &str,
    describe: &ToolDescribeDocument,
) -> Result<Map<String, Value>, String> {
    let mut params = Map::new();
    for (name, value) in &step.tool_params {
        let resolved = match value {
            crate::ToolParamValue::Literal(literal) => literal.clone(),
            crate::ToolParamValue::Variable(variable) => {
                let Some(resolved) = lookup_action_variable(data, variable) else {
                    return Err(format!(
                        "Action '{}' tool param '{}' references missing variable '{}'.",
                        action_name, name, variable
                    ));
                };
                if !(resolved.is_string() || resolved.is_boolean() || resolved.is_number()) {
                    return Err(format!(
                        "Action '{}' tool param '{}' references variable '{}', which resolved to a non-scalar value.",
                        action_name, name, variable
                    ));
                }
                resolved.clone()
            }
        };

        let expected = describe.params.get(name).ok_or_else(|| {
            format!(
                "Action '{}' references unknown tool param '{}'.",
                action_name, name
            )
        })?;
        if !json_value_matches_declared_type(&resolved, expected.kind.as_str()) {
            return Err(format!(
                "Action '{}' tool param '{}' resolved to {}, but the tool expects {}.",
                action_name,
                name,
                describe_runtime_value(&resolved),
                display_type_name(expected.kind.as_str())
            ));
        }

        params.insert(name.clone(), resolved);
    }

    for (name, param_spec) in &describe.params {
        if param_spec.required && param_spec.default.is_none() && !params.contains_key(name) {
            return Err(format!(
                "Action '{}' tool step is missing required param '{}'.",
                action_name, name
            ));
        }
    }

    Ok(params)
}

pub(crate) fn project_tools_root(project_root: &Path) -> PathBuf {
    project_root.join(PROJECT_TOOLS_RELATIVE_PATH)
}

pub(crate) fn machine_tools_root() -> PathBuf {
    crate::config::paths::cargo_ai_root().join("tools")
}

pub(crate) fn bundled_tools_root_for_export(dest_dir: &Path) -> PathBuf {
    dest_dir.join(PROJECT_TOOLS_RELATIVE_PATH)
}

pub(crate) fn tool_bundle_destination_for_export(dest_dir: &Path, tool_id: &str) -> PathBuf {
    bundled_tools_root_for_export(dest_dir).join(tool_id)
}

pub(crate) fn copy_tool_bundle_for_export(
    resolved: &ResolvedTool,
    dest_dir: &Path,
    force_overwrite: bool,
) -> io::Result<()> {
    let destination = tool_bundle_destination_for_export(dest_dir, &resolved.tool_id);
    if destination.exists() {
        if !force_overwrite {
            return Err(io::Error::new(
                ErrorKind::AlreadyExists,
                format!(
                    "Bundled tool output already exists at '{}'. Re-run with --force to overwrite.",
                    destination.display()
                ),
            ));
        }
        fs::remove_dir_all(&destination)?;
    }

    copy_directory_recursive(&resolved.tool_dir, &destination)
}

pub(crate) fn scaffold_local_tool(project_root: &Path, tool_name: &str) -> Result<(), String> {
    validate_local_tool_name(tool_name)?;
    if !project_root.join(PROJECT_METADATA_RELATIVE_PATH).exists() {
        return Err(format!(
            "No Cargo AI project metadata found at '{}'. Run `cargo ai init` first.",
            project_root.join(PROJECT_METADATA_RELATIVE_PATH).display()
        ));
    }

    let source_dir = project_root.join("tools").join(tool_name);
    let tool_dir = project_tools_root(project_root).join(tool_name);
    let cargo_toml_path = source_dir.join("Cargo.toml");
    let main_rs_path = source_dir.join("src").join("main.rs");
    let lib_rs_path = source_dir.join("src").join("lib.rs");
    let manifest_path = tool_dir.join(TOOL_MANIFEST_FILE_NAME);

    for path in [
        &cargo_toml_path,
        &main_rs_path,
        &lib_rs_path,
        &manifest_path,
    ] {
        if path.exists() {
            return Err(format!(
                "Tool scaffold would overwrite existing managed file '{}'.",
                path.display()
            ));
        }
    }

    fs::create_dir_all(source_dir.join("src")).map_err(|error| {
        format!(
            "Failed to create tool source directory '{}': {}",
            source_dir.display(),
            error
        )
    })?;
    fs::create_dir_all(&tool_dir).map_err(|error| {
        format!(
            "Failed to create tool metadata directory '{}': {}",
            tool_dir.display(),
            error
        )
    })?;

    let module_name = tool_name.replace('-', "_");
    write_utf8_file(
        &cargo_toml_path,
        TOOL_SCAFFOLD_CARGO_TOML.replace("__TOOL_NAME__", tool_name),
    )?;
    write_utf8_file(
        &main_rs_path,
        TOOL_SCAFFOLD_MAIN_RS.replace("__TOOL_MODULE__", module_name.as_str()),
    )?;
    write_utf8_file(
        &lib_rs_path,
        TOOL_SCAFFOLD_LIB_RS.replace("__TOOL_NAME__", tool_name),
    )?;
    write_utf8_file(
        &manifest_path,
        render_source_tool_manifest_json(
            tool_name,
            format!("tools/{tool_name}/Cargo.toml").as_str(),
            tool_name,
        ),
    )?;

    Ok(())
}

pub(crate) fn build_source_tool(
    tool_name: &str,
    build_target: &crate::agent_builder::build_target::BuildTarget,
    scope: ToolScope,
    project_root: &Path,
) -> Result<ResolvedTool, String> {
    if scope == ToolScope::Bundled {
        return Err("Bundled scope is not supported for `cargo ai tools build`.".to_string());
    }

    let source_manifest_path = project_tools_root(project_root)
        .join(tool_name)
        .join(TOOL_MANIFEST_FILE_NAME);
    let source_manifest = load_tool_manifest(&source_manifest_path, tool_name)?;
    let source = source_manifest.source.as_ref().ok_or_else(|| {
        format!(
            "Tool '{}' is not source-backed and cannot be built with `cargo ai tools build`.",
            tool_name
        )
    })?;
    let manifest_path = project_root.join(source.manifest_path.as_str());
    if !manifest_path.exists() {
        return Err(format!(
            "Tool '{}' source manifest '{}' was not found.",
            tool_name,
            manifest_path.display()
        ));
    }

    let source_dir = manifest_path.parent().ok_or_else(|| {
        format!(
            "Tool '{}' manifest path has no parent directory.",
            tool_name
        )
    })?;
    let binary_name = default_binary_name_for_manifest(&source_manifest);

    let mut command = Command::new("cargo");
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .current_dir(source_dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(target) = build_target.cargo_target() {
        command.arg("--target").arg(target);
    }

    let status = command.status().map_err(|error| {
        format!(
            "Failed to run cargo build for tool '{}': {}",
            tool_name, error
        )
    })?;
    if !status.success() {
        return Err(format!("Cargo build failed for tool '{}'.", tool_name));
    }

    let built_binary_path = build_target.compiled_binary_path(source_dir, binary_name.as_str());
    if !built_binary_path.exists() {
        return Err(format!(
            "Expected built tool binary '{}' at '{}', but it was not produced.",
            binary_name,
            built_binary_path.display()
        ));
    }

    let scope_root = match scope {
        ToolScope::Project => project_tools_root(project_root),
        ToolScope::Machine => machine_tools_root(),
        ToolScope::Bundled => unreachable!(),
    };
    let tool_dir = scope_root.join(tool_name);
    let artifact_relative_path = PathBuf::from("bin")
        .join(build_target.cache_key_target())
        .join(&binary_name);
    let artifact_path = tool_dir.join(&artifact_relative_path);
    if let Some(parent) = artifact_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create tool artifact directory '{}': {}",
                parent.display(),
                error
            )
        })?;
    }
    fs::copy(&built_binary_path, &artifact_path).map_err(|error| {
        format!(
            "Failed to copy built tool binary into '{}': {}",
            artifact_path.display(),
            error
        )
    })?;

    let manifest_json = match scope {
        ToolScope::Project => render_project_built_tool_manifest_json(
            tool_name,
            source.manifest_path.as_str(),
            binary_name.as_str(),
            build_target.cache_key_target(),
            artifact_relative_path.to_string_lossy().as_ref(),
        ),
        ToolScope::Machine => render_binary_tool_manifest_json(
            tool_name,
            binary_name.as_str(),
            build_target.cache_key_target(),
            artifact_relative_path.to_string_lossy().as_ref(),
        ),
        ToolScope::Bundled => unreachable!(),
    };
    let manifest_output_path = tool_dir.join(TOOL_MANIFEST_FILE_NAME);
    if let Some(parent) = manifest_output_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create tool manifest directory '{}': {}",
                parent.display(),
                error
            )
        })?;
    }
    write_utf8_file(&manifest_output_path, manifest_json)?;

    ToolResolver::new(
        if scope == ToolScope::Project {
            Some(project_root.to_path_buf())
        } else {
            None
        },
        build_target.cache_key_target(),
    )
    .resolve_tool(tool_name)
}

pub(crate) fn run(sub_m: &ArgMatches) -> bool {
    if let Some(build_m) = sub_m.subcommand_matches("build") {
        run_build(build_m)
    } else if let Some(describe_m) = sub_m.subcommand_matches("describe") {
        run_describe(describe_m)
    } else if let Some(check_m) = sub_m.subcommand_matches("check") {
        run_check(check_m)
    } else {
        eprintln!(
            "No tools subcommand found. Try `cargo ai tools build <name>`, `cargo ai tools describe <name>`, or `cargo ai tools check <name>`."
        );
        false
    }
}

fn run_build(sub_m: &ArgMatches) -> bool {
    let Some(name) = sub_m.get_one::<String>("name").map(String::as_str) else {
        eprintln!("x Missing tool name. Use `cargo ai tools build <name>`.");
        return false;
    };
    let project_root = match std::env::current_dir()
        .ok()
        .and_then(|dir| maybe_find_project_root(dir.as_path()))
    {
        Some(root) => root,
        None => {
            eprintln!(
                "x No Cargo AI project metadata was found from the current directory upward."
            );
            return false;
        }
    };

    let build_target = match crate::agent_builder::build_target::BuildTarget::from_cli(
        sub_m.get_one::<String>("target").map(String::as_str),
    ) {
        Ok(target) => target,
        Err(error) => {
            eprintln!("x {error}");
            return false;
        }
    };
    let scope = match sub_m
        .get_one::<String>("scope")
        .map(String::as_str)
        .unwrap_or("project")
    {
        "project" => ToolScope::Project,
        "machine" => ToolScope::Machine,
        other => {
            eprintln!(
                "x Unsupported tool scope '{}'. Use `project` or `machine`.",
                other
            );
            return false;
        }
    };

    match build_source_tool(name, &build_target, scope, &project_root) {
        Ok(resolved) => {
            println!("✓ Tool built");
            println!("Tool:   {}", resolved.tool_id);
            println!("Scope:  {}", display_scope(&resolved.scope));
            println!("Target: {}", resolved.target_triple);
            println!("Path:   {}", resolved.binary_path.display());
            true
        }
        Err(error) => {
            eprintln!("x {error}");
            false
        }
    }
}

fn run_describe(sub_m: &ArgMatches) -> bool {
    let Some(name) = sub_m.get_one::<String>("name").map(String::as_str) else {
        eprintln!("x Missing tool name. Use `cargo ai tools describe <name>`.");
        return false;
    };

    match resolver_from_current_dir(sub_m).and_then(|resolver| resolver.resolve_contract(name)) {
        Ok(contract) => {
            match serde_json::to_string_pretty(&contract.describe) {
                Ok(rendered) => println!("{rendered}"),
                Err(error) => {
                    eprintln!("x Failed to render tool describe output: {error}");
                    return false;
                }
            }
            true
        }
        Err(error) => {
            eprintln!("x {error}");
            false
        }
    }
}

fn run_check(sub_m: &ArgMatches) -> bool {
    if let Some(config) = sub_m.get_one::<String>("config").map(String::as_str) {
        let definition = match crate::runtime_definition::RuntimeAgentDefinition::load_from_path(
            Path::new(config),
        ) {
            Ok(definition) => definition,
            Err(error) => {
                eprintln!("x {error}");
                return false;
            }
        };
        let target_triple = crate::cargo_ai_metadata::current_build_target();
        let project_root = std::env::current_dir()
            .ok()
            .and_then(|_| maybe_find_project_root(Path::new(config)));
        let resolver = ToolResolver::new(project_root, target_triple);
        match audit_actions_for_tools(&definition.actions(), &resolver, current_platform_label()) {
            Ok(resolved) => {
                println!("✓ Tool contract checks passed");
                println!("Tools: {}", resolved.len());
                for tool in resolved {
                    println!(
                        "- {} ({}, {})",
                        tool.tool_id,
                        display_scope(&tool.scope),
                        tool.target_triple
                    );
                }
                true
            }
            Err(error) => {
                eprintln!("x {error}");
                false
            }
        }
    } else if let Some(name) = sub_m.get_one::<String>("name").map(String::as_str) {
        match resolver_from_current_dir(sub_m).and_then(|resolver| resolver.resolve_contract(name))
        {
            Ok(contract) => {
                println!("✓ Tool contract checks passed");
                println!("Tool:   {}", contract.resolved.tool_id);
                println!("Scope:  {}", display_scope(&contract.resolved.scope));
                println!("Target: {}", contract.resolved.target_triple);
                true
            }
            Err(error) => {
                eprintln!("x {error}");
                false
            }
        }
    } else {
        eprintln!("x Missing tool name or --config for `cargo ai tools check`.");
        false
    }
}

fn resolver_from_current_dir(sub_m: &ArgMatches) -> Result<ToolResolver, String> {
    let current_dir = std::env::current_dir()
        .map_err(|error| format!("Failed to read current directory: {error}"))?;
    let project_root = maybe_find_project_root(&current_dir);
    let target_triple = sub_m
        .get_one::<String>("target")
        .cloned()
        .unwrap_or_else(crate::cargo_ai_metadata::current_build_target);
    Ok(ToolResolver::new(project_root, target_triple))
}

fn resolve_tool_from_scope_root(
    scope_root: &Path,
    scope: ToolScope,
    tool_id: &str,
    target_triple: &str,
) -> Result<Option<ResolvedTool>, String> {
    let tool_dir = scope_root.join(tool_id);
    if !tool_dir.exists() {
        return Ok(None);
    }

    let manifest_path = tool_dir.join(TOOL_MANIFEST_FILE_NAME);
    if !manifest_path.exists() {
        return Err(format!(
            "{} tool '{}' is missing '{}'.",
            display_scope(&scope),
            tool_id,
            manifest_path.display()
        ));
    }

    let manifest = load_tool_manifest(&manifest_path, tool_id)?;
    let artifact = manifest.artifacts.get(target_triple).ok_or_else(|| {
        format!(
            "Tool '{}' does not have a materialized artifact for target '{}'.",
            tool_id, target_triple
        )
    })?;
    let binary_name = default_binary_name_for_manifest(&manifest);
    let binary_path = tool_dir.join(&artifact.path);
    if !binary_path.exists() {
        return Err(format!(
            "Tool '{}' artifact '{}' does not exist on disk.",
            tool_id,
            binary_path.display()
        ));
    }

    Ok(Some(ResolvedTool {
        tool_id: tool_id.to_string(),
        scope,
        tool_dir,
        manifest_path,
        binary_name,
        target_triple: target_triple.to_string(),
        binary_path,
    }))
}

fn load_tool_manifest(path: &Path, expected_tool_id: &str) -> Result<ToolManifest, String> {
    let contents = fs::read_to_string(path).map_err(|error| {
        format!(
            "Failed to read tool manifest '{}': {}",
            path.display(),
            error
        )
    })?;
    let manifest: ToolManifest = serde_json::from_str(&contents).map_err(|error| {
        format!(
            "Failed to parse tool manifest '{}': {}",
            path.display(),
            error
        )
    })?;
    if manifest.schema_version != 1 {
        return Err(format!(
            "Tool manifest '{}' uses unsupported schema_version {}.",
            path.display(),
            manifest.schema_version
        ));
    }
    if manifest.tool_id != expected_tool_id {
        return Err(format!(
            "Tool manifest '{}' declares tool_id '{}', expected '{}'.",
            path.display(),
            manifest.tool_id,
            expected_tool_id
        ));
    }
    Ok(manifest)
}

fn load_tool_describe_document(resolved: &ResolvedTool) -> Result<ToolDescribeDocument, String> {
    let stdout = run_tool_command_capture_stdout(&resolved.binary_path, "describe", None)?;
    serde_json::from_slice::<ToolDescribeDocument>(&stdout).map_err(|error| {
        format!(
            "Tool '{}' returned invalid describe JSON: {}",
            resolved.tool_id, error
        )
    })
}

fn run_tool_command_capture_stdout(
    binary_path: &Path,
    command: &str,
    stdin: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let mut child = Command::new(binary_path);
    child
        .arg(command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin.is_some() {
        child.stdin(Stdio::piped());
    }

    let mut child = child.spawn().map_err(|error| {
        format!(
            "Failed to start tool binary '{}': {}",
            binary_path.display(),
            error
        )
    })?;

    if let Some(stdin_bytes) = stdin {
        use std::io::Write;
        let mut writer = child.stdin.take().ok_or_else(|| {
            format!(
                "Failed to open stdin for tool binary '{}'.",
                binary_path.display()
            )
        })?;
        writer.write_all(stdin_bytes).map_err(|error| {
            format!(
                "Failed to write stdin for tool binary '{}': {}",
                binary_path.display(),
                error
            )
        })?;
    }

    let output = child.wait_with_output().map_err(|error| {
        format!(
            "Failed while waiting for tool binary '{}': {}",
            binary_path.display(),
            error
        )
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!(
                "Tool binary '{}' exited with status {}.",
                binary_path.display(),
                output.status
            )
        } else {
            format!(
                "Tool binary '{}' exited with status {}: {}",
                binary_path.display(),
                output.status,
                stderr
            )
        });
    }

    Ok(output.stdout)
}

fn validate_describe_document(
    describe: &ToolDescribeDocument,
    resolved: &ResolvedTool,
) -> Result<(), String> {
    if describe.protocol_version != TOOL_PROTOCOL_VERSION {
        return Err(format!(
            "Tool '{}' reports unsupported protocol_version {} in describe.",
            resolved.tool_id, describe.protocol_version
        ));
    }
    if describe.name.trim().is_empty() {
        return Err(format!(
            "Tool '{}' describe output is missing a non-empty `name`.",
            resolved.tool_id
        ));
    }
    if describe.description.trim().is_empty() {
        return Err(format!(
            "Tool '{}' describe output is missing a non-empty `description`.",
            resolved.tool_id
        ));
    }
    if describe.result.kind != "string" {
        return Err(format!(
            "Tool '{}' describe result.type must be `string`.",
            resolved.tool_id
        ));
    }

    for (name, param) in &describe.params {
        if !matches!(
            param.kind.as_str(),
            "string" | "boolean" | "integer" | "number"
        ) {
            return Err(format!(
                "Tool '{}' describe param '{}' uses unsupported type '{}'.",
                resolved.tool_id, name, param.kind
            ));
        }
        if param.required && param.default.is_some() {
            return Err(format!(
                "Tool '{}' describe param '{}' cannot be both required and defaulted.",
                resolved.tool_id, name
            ));
        }
        if let Some(default) = param.default.as_ref() {
            if !json_value_matches_declared_type(default, param.kind.as_str()) {
                return Err(format!(
                    "Tool '{}' describe param '{}' default does not match declared type '{}'.",
                    resolved.tool_id, name, param.kind
                ));
            }
        }
    }

    for (label, value) in [
        ("network", describe.resource_profile.network.as_str()),
        (
            "filesystem_read",
            describe.resource_profile.filesystem_read.as_str(),
        ),
        (
            "filesystem_write",
            describe.resource_profile.filesystem_write.as_str(),
        ),
        ("subprocess", describe.resource_profile.subprocess.as_str()),
        ("env_read", describe.resource_profile.env_read.as_str()),
        (
            "credential_access",
            describe.resource_profile.credential_access.as_str(),
        ),
    ] {
        if !matches!(value, "none" | "optional" | "required") {
            return Err(format!(
                "Tool '{}' describe resource_profile.{} uses unsupported value '{}'.",
                resolved.tool_id, label, value
            ));
        }
    }

    for (label, example) in [
        ("minimal_invoke", &describe.examples.minimal_invoke),
        ("full_invoke", &describe.examples.full_invoke),
    ] {
        let Some(example_obj) = example.as_object() else {
            return Err(format!(
                "Tool '{}' describe examples.{} must be an object.",
                resolved.tool_id, label
            ));
        };
        let protocol_version = example_obj
            .get("protocol_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                format!(
                    "Tool '{}' describe examples.{} is missing integer protocol_version.",
                    resolved.tool_id, label
                )
            })?;
        if protocol_version != TOOL_PROTOCOL_VERSION as u64 {
            return Err(format!(
                "Tool '{}' describe examples.{} must use protocol_version {}.",
                resolved.tool_id, label, TOOL_PROTOCOL_VERSION
            ));
        }
        if !example_obj.get("params").is_some_and(Value::is_object) {
            return Err(format!(
                "Tool '{}' describe examples.{} must include an object `params` field.",
                resolved.tool_id, label
            ));
        }
    }

    Ok(())
}

pub(crate) fn validate_tool_invoke_response(
    resolved: &ResolvedTool,
    stdout: &[u8],
) -> Result<Option<String>, String> {
    let response: ToolInvokeResponse = serde_json::from_slice(stdout).map_err(|error| {
        format!(
            "Tool '{}' returned invalid invoke JSON: {}",
            resolved.tool_id, error
        )
    })?;
    if response.protocol_version != TOOL_PROTOCOL_VERSION {
        return Err(format!(
            "Tool '{}' returned unsupported invoke protocol_version {}.",
            resolved.tool_id, response.protocol_version
        ));
    }
    Ok(response.result)
}

fn json_value_matches_declared_type(value: &Value, expected: &str) -> bool {
    match expected {
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        _ => false,
    }
}

fn display_type_name(kind: &str) -> &str {
    match kind {
        "string" => "a string",
        "boolean" => "a boolean",
        "integer" => "an integer",
        "number" => "a number",
        _ => "a supported scalar value",
    }
}

fn describe_runtime_value(value: &Value) -> &'static str {
    if value.is_string() {
        "a string"
    } else if value.is_boolean() {
        "a boolean"
    } else if value.as_i64().is_some() || value.as_u64().is_some() {
        "an integer"
    } else if value.is_number() {
        "a number"
    } else if value.is_array() {
        "an array"
    } else if value.is_object() {
        "an object"
    } else {
        "null"
    }
}

fn validate_tool_identifier(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Tool names cannot be empty.".to_string());
    }
    if name != name.trim() {
        return Err("Tool names cannot start or end with whitespace.".to_string());
    }
    if name.chars().any(char::is_whitespace) {
        return Err("Tool names cannot contain whitespace.".to_string());
    }
    Ok(())
}

fn validate_local_tool_name(name: &str) -> Result<(), String> {
    validate_tool_identifier(name)?;
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(
            "Local tool scaffold names may use only ASCII letters, numbers, '_' or '-'."
                .to_string(),
        );
    }
    Ok(())
}

fn write_utf8_file(path: &Path, contents: String) -> Result<(), String> {
    fs::write(path, contents)
        .map_err(|error| format!("Failed to write '{}': {}", path.display(), error))
}

fn render_source_tool_manifest_json(
    tool_id: &str,
    manifest_path: &str,
    binary_name: &str,
) -> String {
    serde_json::json!({
        "schema_version": 1,
        "tool_id": tool_id,
        "source": {
            "manifest_path": manifest_path
        },
        "binary": {
            "default_name": binary_name
        },
        "artifacts": {}
    })
    .to_string()
}

fn render_project_built_tool_manifest_json(
    tool_id: &str,
    manifest_path: &str,
    binary_name: &str,
    target_triple: &str,
    artifact_path: &str,
) -> String {
    serde_json::json!({
        "schema_version": 1,
        "tool_id": tool_id,
        "source": {
            "manifest_path": manifest_path
        },
        "binary": {
            "default_name": binary_name
        },
        "artifacts": {
            target_triple: {
                "path": artifact_path
            }
        }
    })
    .to_string()
}

fn render_binary_tool_manifest_json(
    tool_id: &str,
    binary_name: &str,
    target_triple: &str,
    artifact_path: &str,
) -> String {
    serde_json::json!({
        "schema_version": 1,
        "tool_id": tool_id,
        "binary": {
            "default_name": binary_name
        },
        "artifacts": {
            target_triple: {
                "path": artifact_path
            }
        }
    })
    .to_string()
}

fn default_binary_name_for_manifest(manifest: &ToolManifest) -> String {
    manifest
        .binary
        .default_name
        .clone()
        .unwrap_or_else(|| manifest.tool_id.clone())
}

fn display_scope(scope: &ToolScope) -> &'static str {
    match scope {
        ToolScope::Bundled => "bundled",
        ToolScope::Project => "project",
        ToolScope::Machine => "machine",
    }
}

fn lookup_action_variable<'a>(data: &'a Value, variable: &str) -> Option<&'a Value> {
    if let Some(runtime_name) = variable.strip_prefix("runtime.") {
        return data
            .get("runtime")
            .and_then(Value::as_object)
            .and_then(|runtime| runtime.get(runtime_name));
    }

    if variable.contains('.') {
        return None;
    }

    data.get(variable)
}

fn tool_step_context(action_name: Option<&str>, step_index: Option<usize>) -> String {
    match (action_name, step_index) {
        (Some(action_name), Some(step_index)) => {
            format!("Action '{}' tool step {}", action_name, step_index + 1)
        }
        (Some(action_name), None) => format!("Action '{}' tool step", action_name),
        _ => "Tool step".to_string(),
    }
}

fn current_platform_label() -> Option<&'static str> {
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

fn step_matches_platform(platforms: Option<&[String]>, current_platform: Option<&str>) -> bool {
    match platforms {
        None => true,
        Some(platforms) => current_platform
            .is_some_and(|platform| platforms.iter().any(|candidate| candidate == platform)),
    }
}

fn copy_directory_recursive(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory_recursive(&source_path, &destination_path)?;
        } else {
            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        maybe_find_project_root, render_binary_tool_manifest_json,
        render_source_tool_manifest_json, validate_local_tool_name,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(stem: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("cargo-ai-tools-test-{stem}-{nanos}"))
    }

    #[test]
    fn finds_project_root_by_managed_metadata() {
        let root = temp_dir("project-root");
        let nested = root.join("examples").join("agents");
        fs::create_dir_all(root.join(".cargo-ai")).expect("project metadata dir should be created");
        fs::write(root.join(".cargo-ai/project.toml"), "format_version = 1\n")
            .expect("project metadata should be written");
        fs::create_dir_all(&nested).expect("nested dir should be created");

        let found = maybe_find_project_root(&nested).expect("project root should be found");
        assert_eq!(found, root);

        let _ = fs::remove_dir_all(found);
    }

    #[test]
    fn local_tool_name_validation_rejects_whitespace() {
        let err = validate_local_tool_name("bad name").expect_err("whitespace should fail");
        assert!(err.contains("whitespace"));
    }

    #[test]
    fn source_manifest_renders_expected_shape() {
        let rendered = render_source_tool_manifest_json(
            "render_cover_image",
            "tools/render_cover_image/Cargo.toml",
            "render_cover_image",
        );
        assert!(rendered.contains("\"tool_id\":\"render_cover_image\""));
        assert!(rendered.contains("\"manifest_path\":\"tools/render_cover_image/Cargo.toml\""));
    }

    #[test]
    fn binary_manifest_renders_target_artifact() {
        let rendered = render_binary_tool_manifest_json(
            "render_cover_image",
            "render_cover_image",
            "aarch64-apple-darwin",
            "bin/aarch64-apple-darwin/render_cover_image",
        );
        assert!(rendered.contains("\"aarch64-apple-darwin\""));
        assert!(rendered.contains("\"default_name\":\"render_cover_image\""));
    }
}
