//! Runtime behavior for account-backed package management.
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use clap::ArgMatches;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Cursor, Write};
use std::path::{Component, Path, PathBuf};
use tar::{Archive, Builder, EntryType, HeaderMode};
use uuid::Uuid;

use crate::infra_api;
use crate::ui;

use super::helpers::{
    apply_projects_list_display_limit, load_account_auth, persist_refreshed_access_token,
    refresh_access_token_for_retry, RefreshAccessError, INFRA_BASE_URL,
};

#[derive(Clone, Debug)]
enum ProjectsCommand {
    List {
        owner_handle: Option<String>,
        include_archived: bool,
        display_limit: Option<usize>,
    },
    #[cfg(feature = "developer-tools")]
    Publish {
        profile: String,
    },
    Pull {
        name: String,
        owner_handle: Option<String>,
        version: Option<String>,
        output_dir: Option<PathBuf>,
        force: bool,
    },
    Visibility {
        name: String,
        is_public: bool,
    },
    Archive {
        name: String,
        is_archived: bool,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PackageArchiveDocument {
    format_version: u32,
    entries: Vec<PackageArchiveEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PackageArchiveEntry {
    path: String,
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    contents_base64: Option<String>,
}

#[cfg(feature = "developer-tools")]
#[derive(Clone, Debug)]
struct PublishPayload {
    project_name: String,
    project_version: String,
    package_manifest: Value,
    package_sha256: String,
    package_size_bytes: i64,
    package_archive_base64: String,
}

const PULLED_PROJECT_METADATA_RELATIVE_PATH: &str = ".cargo-ai/project.toml";
const PACKAGE_MANIFEST_FILE_NAME: &str = "cargo-ai-package.toml";
const PULLED_PACKAGE_RECEIPT_RELATIVE_PATH: &str = ".cargo-ai/origin/cargo-ai-package.toml";
const ESTIMATED_PUBLISH_ACCESS_TOKEN: &str = "__publish-size-estimate__";
#[cfg(feature = "developer-tools")]
const SAFE_PROJECT_PUBLISH_REQUEST_LIMIT_BYTES: u64 = 5_500_000;

pub async fn run(projects_m: &ArgMatches) -> bool {
    let projects_command = if let Some(list_m) = projects_m.subcommand_matches("list") {
        ProjectsCommand::List {
            owner_handle: list_m
                .get_one::<String>("owner_handle")
                .map(|s| s.to_string()),
            include_archived: list_m.get_flag("include_archived"),
            display_limit: if list_m.get_flag("all") {
                None
            } else {
                Some(
                    list_m
                        .get_one::<u32>("limit")
                        .copied()
                        .unwrap_or(20)
                        .try_into()
                        .expect("u32 limit should fit in usize"),
                )
            },
        }
    } else if let Some(pull_m) = projects_m.subcommand_matches("pull") {
        let name = pull_m
            .get_one::<String>("name")
            .or_else(|| pull_m.get_one::<String>("name_positional"))
            .map(String::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if name.is_empty() {
            eprintln!("x Missing package name. Provide NAME or --name <NAME>.");
            return false;
        }

        ProjectsCommand::Pull {
            name,
            owner_handle: pull_m
                .get_one::<String>("owner_handle")
                .map(|s| s.to_string()),
            version: pull_m.get_one::<String>("version").map(|s| s.to_string()),
            output_dir: match pull_m.get_one::<String>("output_dir") {
                Some(raw) if raw.trim().is_empty() => {
                    eprintln!("x Output directory cannot be empty. Provide --output-dir <DIR>.");
                    return false;
                }
                Some(raw) => Some(PathBuf::from(raw)),
                None => None,
            },
            force: pull_m.get_flag("force"),
        }
    } else if let Some(visibility_m) = projects_m.subcommand_matches("visibility") {
        let Some(name) = visibility_m.get_one::<String>("name") else {
            eprintln!("x Missing package name. Provide --name <NAME>.");
            return false;
        };

        ProjectsCommand::Visibility {
            name: name.to_string(),
            is_public: visibility_m.get_flag("public"),
        }
    } else if let Some(archive_m) = projects_m.subcommand_matches("archive") {
        let Some(name) = archive_m.get_one::<String>("name") else {
            eprintln!("x Missing package name. Provide --name <NAME>.");
            return false;
        };

        ProjectsCommand::Archive {
            name: name.to_string(),
            is_archived: archive_m.get_flag("archive"),
        }
    } else {
        #[cfg(feature = "developer-tools")]
        if let Some(publish_m) = projects_m.subcommand_matches("publish") {
            ProjectsCommand::Publish {
                profile: publish_m
                    .get_one::<String>("profile")
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "default".to_string()),
            }
        } else {
            eprintln!(
                "No packages subcommand found. Try 'cargo ai packages list|publish|pull|visibility|archive'."
            );
            return false;
        }

        #[cfg(not(feature = "developer-tools"))]
        {
            eprintln!(
                "No packages subcommand found. Try 'cargo ai packages list|pull|visibility|archive'."
            );
            return false;
        }
    };

    let auth = match load_account_auth() {
        Ok(auth) => auth,
        Err(message) => {
            eprintln!("{}", ui::account_status::normalize_leading_glyph(&message));
            return false;
        }
    };
    let access_token_owned = auth.access_token;
    let refresh_token = auth.refresh_token;

    #[cfg(feature = "developer-tools")]
    let mut prepared_publish_payload: Option<PublishPayload> = None;

    #[cfg(feature = "developer-tools")]
    if let ProjectsCommand::Publish { profile } = &projects_command {
        match prepare_publish_payload(profile.as_str()) {
            Ok(payload) => prepared_publish_payload = Some(payload),
            Err(error) => {
                eprintln!("x {error}");
                return false;
            }
        }
    }

    let mut response = match &projects_command {
        ProjectsCommand::List {
            owner_handle,
            include_archived,
            ..
        } => match infra_api::account::projects::list_projects(
            INFRA_BASE_URL,
            access_token_owned.as_str(),
            owner_handle.as_deref(),
            *include_archived,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("x Request failed: {e:?}");
                return false;
            }
        },
        #[cfg(feature = "developer-tools")]
        ProjectsCommand::Publish { .. } => {
            let payload = prepared_publish_payload
                .as_ref()
                .expect("publish payload should be prepared");

            match infra_api::account::projects::publish_project(
                INFRA_BASE_URL,
                access_token_owned.as_str(),
                payload.project_name.as_str(),
                payload.project_version.as_str(),
                payload.package_manifest.clone(),
                payload.package_sha256.as_str(),
                payload.package_size_bytes,
                payload.package_archive_base64.as_str(),
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("x Request failed: {e:?}");
                    return false;
                }
            }
        }
        ProjectsCommand::Pull {
            name,
            owner_handle,
            version,
            ..
        } => match infra_api::account::projects::pull_project(
            INFRA_BASE_URL,
            access_token_owned.as_str(),
            name,
            owner_handle.as_deref(),
            version.as_deref(),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("x Request failed: {e:?}");
                return false;
            }
        },
        ProjectsCommand::Visibility { name, is_public } => {
            match infra_api::account::projects::set_project_visibility(
                INFRA_BASE_URL,
                access_token_owned.as_str(),
                name,
                *is_public,
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("x Request failed: {e:?}");
                    return false;
                }
            }
        }
        ProjectsCommand::Archive { name, is_archived } => {
            match infra_api::account::projects::set_project_archive(
                INFRA_BASE_URL,
                access_token_owned.as_str(),
                name,
                *is_archived,
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("x Request failed: {e:?}");
                    return false;
                }
            }
        }
    };

    let is_expired_error = response
        .get("type")
        .and_then(|v| v.as_str())
        .map(|t| t == "access_token_expired")
        .unwrap_or(false);

    if is_expired_error {
        match refresh_access_token_for_retry(access_token_owned.as_str(), refresh_token.as_deref())
            .await
        {
            Err(RefreshAccessError::MissingRefreshToken) => {
                eprintln!("! Access token expired, and no refresh token exists in credential store. Run `cargo ai account status` or re-confirm account.");
                render_account_projects_response(&response);
                return false;
            }
            Err(RefreshAccessError::RequestFailed(error)) => {
                eprintln!("x Request failed while refreshing session: {error}");
                return false;
            }
            Err(RefreshAccessError::MissingRefreshedToken(refresh_response)) => {
                eprintln!("! Session refresh did not return a new access token. Cannot retry projects request.");
                render_account_projects_response(&refresh_response);
                return false;
            }
            Ok((retry_access_token, refreshed_expires_in)) => {
                if let Some(rt) = refresh_token.as_deref() {
                    persist_refreshed_access_token(
                        retry_access_token.as_str(),
                        rt,
                        refreshed_expires_in,
                    );
                }

                response = match &projects_command {
                    ProjectsCommand::List {
                        owner_handle,
                        include_archived,
                        ..
                    } => match infra_api::account::projects::list_projects(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        owner_handle.as_deref(),
                        *include_archived,
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("x Request failed after session refresh: {e:?}");
                            return false;
                        }
                    },
                    #[cfg(feature = "developer-tools")]
                    ProjectsCommand::Publish { .. } => {
                        let payload = prepared_publish_payload
                            .as_ref()
                            .expect("publish payload should be prepared");

                        match infra_api::account::projects::publish_project(
                            INFRA_BASE_URL,
                            retry_access_token.as_str(),
                            payload.project_name.as_str(),
                            payload.project_version.as_str(),
                            payload.package_manifest.clone(),
                            payload.package_sha256.as_str(),
                            payload.package_size_bytes,
                            payload.package_archive_base64.as_str(),
                        )
                        .await
                        {
                            Ok(r) => r,
                            Err(e) => {
                                eprintln!("x Request failed after session refresh: {e:?}");
                                return false;
                            }
                        }
                    }
                    ProjectsCommand::Pull {
                        name,
                        owner_handle,
                        version,
                        ..
                    } => match infra_api::account::projects::pull_project(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        name,
                        owner_handle.as_deref(),
                        version.as_deref(),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("x Request failed after session refresh: {e:?}");
                            return false;
                        }
                    },
                    ProjectsCommand::Visibility { name, is_public } => {
                        match infra_api::account::projects::set_project_visibility(
                            INFRA_BASE_URL,
                            retry_access_token.as_str(),
                            name,
                            *is_public,
                        )
                        .await
                        {
                            Ok(r) => r,
                            Err(e) => {
                                eprintln!("x Request failed after session refresh: {e:?}");
                                return false;
                            }
                        }
                    }
                    ProjectsCommand::Archive { name, is_archived } => {
                        match infra_api::account::projects::set_project_archive(
                            INFRA_BASE_URL,
                            retry_access_token.as_str(),
                            name,
                            *is_archived,
                        )
                        .await
                        {
                            Ok(r) => r,
                            Err(e) => {
                                eprintln!("x Request failed after session refresh: {e:?}");
                                return false;
                            }
                        }
                    }
                };
            }
        }
    }

    if let ProjectsCommand::List { display_limit, .. } = &projects_command {
        let _ = apply_projects_list_display_limit(&mut response, *display_limit);
    }

    if let ProjectsCommand::Pull {
        name,
        version,
        output_dir,
        force,
        ..
    } = &projects_command
    {
        if is_project_pull_success(&response) {
            let output_path = output_dir
                .clone()
                .unwrap_or_else(|| PathBuf::from(name.clone()));

            if let Err(error) = restore_pulled_project(&response, &output_path, *force) {
                eprintln!("x {error}");
                return false;
            }

            response["ui"] = build_local_pull_ui(
                response
                    .get("owner_handle")
                    .and_then(|value| value.as_str())
                    .map(|value| value.to_string()),
                response
                    .get("project")
                    .and_then(|value| value.as_str())
                    .unwrap_or(name),
                response
                    .get("project_version")
                    .and_then(|value| value.as_str())
                    .or(version.as_deref())
                    .unwrap_or("latest"),
                output_path.as_path(),
            );
        }
    }

    render_account_projects_response(&response);
    response
        .get("status")
        .and_then(|v| v.as_str())
        .map(|status| status.eq_ignore_ascii_case("success"))
        .unwrap_or(false)
}

#[cfg(feature = "developer-tools")]
fn prepare_publish_payload(profile_name: &str) -> Result<PublishPayload, String> {
    let project_root = current_project_root().ok_or_else(|| {
        "No Cargo AI project metadata was found from the current directory upward.".to_string()
    })?;
    let staging_output_dir = project_root
        .join("target")
        .join("cargo-ai")
        .join("publish-tmp")
        .join(Uuid::new_v4().to_string());
    let staging_output_dir_raw = staging_output_dir.to_string_lossy().to_string();

    println!("Packaging profile `{profile_name}`...");
    println!("Project: {}", project_root.display());

    let assemble_result = crate::commands::package::assemble_current_project_package(
        profile_name,
        Some(staging_output_dir_raw.as_str()),
        true,
        false,
    );
    let assembled = match assemble_result {
        Ok(assembled) => assembled,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging_output_dir);
            return Err(error);
        }
    };

    let project_name = assembled.manifest_project_name.ok_or_else(|| {
        "Project publish requires `.cargo-ai/project.toml` `[project].name`.".to_string()
    })?;
    let project_version = assembled.manifest_project_version.ok_or_else(|| {
        "Project publish requires `.cargo-ai/project.toml` `[project].version`.".to_string()
    })?;

    Version::parse(project_version.as_str()).map_err(|error| {
        format!(
            "Project version '{}' is not valid semver: {}",
            project_version, error
        )
    })?;

    let archive_bytes = assembled.archive_bytes.clone();
    let package_sha256 = sha256_hex(archive_bytes.as_slice());
    let package_size_bytes = i64::try_from(archive_bytes.len())
        .map_err(|_| "Package archive size exceeded supported limits.".to_string())?;
    let package_archive_base64 = BASE64_STANDARD.encode(archive_bytes.as_slice());
    let estimated_request_size_bytes =
        crate::infra_api::account::projects::estimate_publish_project_request_size(
            ESTIMATED_PUBLISH_ACCESS_TOKEN,
            project_name.as_str(),
            project_version.as_str(),
            assembled.manifest_value.clone(),
            package_sha256.as_str(),
            package_size_bytes,
            package_archive_base64.as_str(),
        )?;

    println!(
        "Package size on disk: {}",
        format_bytes(assembled.assembled_size_bytes)
    );
    println!(
        "Archive size:         {}",
        format_bytes(assembled.archive_size_bytes)
    );
    println!(
        "Estimated request:    {}",
        format_bytes(estimated_request_size_bytes)
    );
    println!();

    let _ = fs::remove_dir_all(&staging_output_dir);

    if estimated_request_size_bytes > SAFE_PROJECT_PUBLISH_REQUEST_LIMIT_BYTES {
        return Err(format!(
            "Estimated publish request size {} exceeds the current safe package-publish ceiling of about {}. Keep packaged assets minimal and remove large sample files before publishing.",
            format_bytes(estimated_request_size_bytes),
            format_bytes(SAFE_PROJECT_PUBLISH_REQUEST_LIMIT_BYTES),
        ));
    }

    Ok(PublishPayload {
        project_name,
        project_version,
        package_manifest: assembled.manifest_value,
        package_sha256,
        package_size_bytes,
        package_archive_base64,
    })
}

fn render_account_projects_response(response: &Value) {
    if !ui::account_status::render_backend_ui(response) {
        match serde_json::to_string_pretty(response) {
            Ok(pretty) => println!("{pretty}"),
            Err(_) => println!("{response:?}"),
        }
    }
}

fn current_project_root() -> Option<PathBuf> {
    std::env::current_dir()
        .ok()
        .and_then(|dir| crate::commands::tools::maybe_find_project_root(dir.as_path()))
}

fn is_project_pull_success(response: &Value) -> bool {
    response
        .get("type")
        .and_then(|v| v.as_str())
        .map(|t| t == "account_projects_pull_succeeded")
        .unwrap_or(false)
}

fn restore_pulled_project(response: &Value, output_path: &Path, force: bool) -> Result<(), String> {
    let archive_base64 = response
        .get("package_archive_base64")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            "Pull succeeded but response did not include `package_archive_base64`.".to_string()
        })?;
    let package_sha256 = response
        .get("package_sha256")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            "Pull succeeded but response did not include `package_sha256`.".to_string()
        })?;
    let package_size_bytes = response
        .get("package_size_bytes")
        .and_then(|value| value.as_i64())
        .ok_or_else(|| {
            "Pull succeeded but response did not include `package_size_bytes`.".to_string()
        })?;

    let archive_bytes = BASE64_STANDARD
        .decode(archive_base64.as_bytes())
        .map_err(|error| format!("Failed to decode package archive: {error}"))?;
    let decoded_size_bytes = i64::try_from(archive_bytes.len())
        .map_err(|_| "Decoded package archive exceeded supported size limits.".to_string())?;
    if decoded_size_bytes != package_size_bytes {
        return Err(format!(
            "Package archive size mismatch. Expected {} bytes, got {} bytes after decoding.",
            package_size_bytes, decoded_size_bytes
        ));
    }

    let decoded_sha256 = sha256_hex(archive_bytes.as_slice());
    if decoded_sha256 != package_sha256 {
        return Err(format!(
            "Package archive checksum mismatch. Expected {}, got {}.",
            package_sha256, decoded_sha256
        ));
    }

    prepare_output_directory(output_path, force)?;
    extract_package_archive_bytes(archive_bytes.as_slice(), output_path)?;
    relocate_pulled_package_receipt(output_path)
}

fn relocate_pulled_package_receipt(project_root: &Path) -> Result<(), String> {
    let project_metadata_path = project_root.join(PULLED_PROJECT_METADATA_RELATIVE_PATH);
    if !project_metadata_path.exists() {
        return Err(format!(
            "Pulled project is missing '{}'.",
            project_metadata_path.display()
        ));
    }

    let root_receipt_path = project_root.join(PACKAGE_MANIFEST_FILE_NAME);
    if !root_receipt_path.exists() {
        return Ok(());
    }

    let origin_receipt_path = project_root.join(PULLED_PACKAGE_RECEIPT_RELATIVE_PATH);
    if let Some(parent) = origin_receipt_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create pulled-project receipt directory '{}': {}",
                parent.display(),
                error
            )
        })?;
    }

    fs::rename(&root_receipt_path, &origin_receipt_path).map_err(|error| {
        format!(
            "Failed to move pulled package receipt from '{}' to '{}': {}",
            root_receipt_path.display(),
            origin_receipt_path.display(),
            error
        )
    })
}

fn prepare_output_directory(path: &Path, force: bool) -> Result<(), String> {
    if path.exists() {
        if !force {
            return Err(format!(
                "Output directory '{}' already exists. Re-run with --force to replace it, or choose --output-dir <DIR>.",
                path.display()
            ));
        }

        if path.is_dir() {
            fs::remove_dir_all(path).map_err(|error| {
                format!(
                    "Failed to replace existing output directory '{}': {}",
                    path.display(),
                    error
                )
            })?;
        } else {
            fs::remove_file(path).map_err(|error| {
                format!(
                    "Failed to replace existing output path '{}': {}",
                    path.display(),
                    error
                )
            })?;
        }
    }

    fs::create_dir_all(path).map_err(|error| {
        format!(
            "Failed to create output directory '{}': {}",
            path.display(),
            error
        )
    })
}

fn build_local_pull_ui(
    owner_handle: Option<String>,
    project_name: &str,
    project_version: &str,
    output_path: &Path,
) -> Value {
    let mut source_items = vec![json!({"label": "Package", "value": project_name})];
    if let Some(owner_handle) = owner_handle {
        source_items.insert(0, json!({"label": "Owner", "value": owner_handle}));
    }

    json!({
        "schema": "1.0",
        "kind": "success",
        "icon": "✓",
        "title": "Package restored",
        "summary": format!("Restored `{project_name}` to `{}`.", display_path(output_path)),
        "sections": [
            {
                "type": "kv",
                "title": "Source",
                "title_style": "plain",
                "layout": "aligned",
                "items": source_items
            },
            {
                "type": "kv",
                "title": "Package",
                "title_style": "plain",
                "layout": "aligned",
                "items": [
                    {"label": "Version", "value": project_version}
                ]
            },
            {
                "type": "kv",
                "title": "Output",
                "title_style": "plain",
                "layout": "aligned",
                "items": [
                    {"label": "Directory", "value": format!("`{}`", display_path(output_path))}
                ]
            },
            {
                "type": "kv",
                "title": "Available commands",
                "title_style": "plain",
                "layout": "aligned",
                "items": [
                    {"label": "Build one tool", "value": "`cargo ai tools build <tool-name>`"},
                    {"label": "Build project", "value": "`cargo ai build`"},
                    {"label": "Package project", "value": "`cargo ai package`"}
                ]
            }
        ]
    })
}

fn display_path(path: &Path) -> String {
    if path.is_relative() {
        return path.display().to_string();
    }

    match std::env::current_dir() {
        Ok(current_dir) => match path.strip_prefix(&current_dir) {
            Ok(relative) if relative.as_os_str().is_empty() => ".".to_string(),
            Ok(relative) => format!("./{}", relative.display()),
            Err(_) => path.display().to_string(),
        },
        Err(_) => path.display().to_string(),
    }
}

pub(crate) fn create_package_archive_bytes(package_root: &Path) -> Result<Vec<u8>, String> {
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut archive_builder = Builder::new(encoder);
    archive_builder.mode(HeaderMode::Deterministic);
    append_compressed_archive_entries(&mut archive_builder, package_root, package_root)?;
    let encoder = archive_builder
        .into_inner()
        .map_err(|error| format!("Failed to finalize compressed project archive: {error}"))?;
    encoder
        .finish()
        .map_err(|error| format!("Failed to finish compressed project archive: {error}"))
}

fn append_compressed_archive_entries<W: Write>(
    archive_builder: &mut Builder<W>,
    package_root: &Path,
    current_path: &Path,
) -> Result<(), String> {
    let mut children = fs::read_dir(current_path)
        .map_err(|error| {
            format!(
                "Failed to read package directory '{}' while building project archive: {}",
                current_path.display(),
                error
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            format!(
                "Failed to read package directory entry under '{}': {}",
                current_path.display(),
                error
            )
        })?;
    children.sort_by_key(|entry| entry.file_name());

    for child in children {
        let child_path = child.path();
        let relative_path = child_path
            .strip_prefix(package_root)
            .map_err(|_| {
                format!(
                    "Packaged path '{}' is not relative to the package root '{}'.",
                    child_path.display(),
                    package_root.display()
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");

        if child_path.is_dir() {
            archive_builder
                .append_dir(relative_path.as_str(), child_path.as_path())
                .map_err(|error| {
                    format!(
                        "Failed to append packaged directory '{}' to the compressed project archive: {}",
                        child_path.display(),
                        error
                    )
                })?;
            append_compressed_archive_entries(archive_builder, package_root, child_path.as_path())?;
        } else {
            let mut file = File::open(child_path.as_path()).map_err(|error| {
                format!(
                    "Failed to read packaged file '{}' while building project archive: {}",
                    child_path.display(),
                    error
                )
            })?;
            archive_builder
                .append_file(relative_path.as_str(), &mut file)
                .map_err(|error| {
                    format!(
                        "Failed to append packaged file '{}' to the compressed project archive: {}",
                        child_path.display(),
                        error
                    )
                })?;
        }
    }

    Ok(())
}

fn extract_package_archive_bytes(archive_bytes: &[u8], output_root: &Path) -> Result<(), String> {
    match extract_compressed_package_archive_bytes(archive_bytes, output_root) {
        Ok(()) => Ok(()),
        Err(compressed_error) => extract_legacy_package_archive_bytes(archive_bytes, output_root)
            .map_err(|legacy_error| {
                format!(
                    "Failed to parse project package archive as a compressed tarball ({compressed_error}) or legacy JSON archive ({legacy_error})."
                )
            }),
    }
}

fn extract_compressed_package_archive_bytes(
    archive_bytes: &[u8],
    output_root: &Path,
) -> Result<(), String> {
    let decoder = GzDecoder::new(Cursor::new(archive_bytes));
    let mut archive = Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| format!("Failed to read compressed project archive entries: {error}"))?;

    for entry in entries {
        let mut entry = entry
            .map_err(|error| format!("Failed to read compressed project archive entry: {error}"))?;
        let entry_path = entry.path().map_err(|error| {
            format!("Failed to read compressed project archive entry path: {error}")
        })?;
        let relative_path = entry_path.to_string_lossy().replace('\\', "/");
        validate_relative_archive_path(relative_path.as_str())?;
        let target_path = output_root.join(relative_path.as_str());
        match entry.header().entry_type() {
            EntryType::Directory => {
                fs::create_dir_all(&target_path).map_err(|error| {
                    format!(
                        "Failed to create restored directory '{}': {}",
                        target_path.display(),
                        error
                    )
                })?;
            }
            EntryType::Regular => {
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent).map_err(|error| {
                        format!(
                            "Failed to create restored parent directory '{}': {}",
                            parent.display(),
                            error
                        )
                    })?;
                }
                entry.unpack(&target_path).map_err(|error| {
                    format!(
                        "Failed to write restored file '{}': {}",
                        target_path.display(),
                        error
                    )
                })?;
            }
            other => {
                return Err(format!(
                    "Compressed archive entry '{}' has unsupported kind '{:?}'.",
                    relative_path, other
                ));
            }
        }
    }

    Ok(())
}

fn extract_legacy_package_archive_bytes(
    archive_bytes: &[u8],
    output_root: &Path,
) -> Result<(), String> {
    let archive: PackageArchiveDocument = serde_json::from_slice(archive_bytes)
        .map_err(|error| format!("Failed to parse project package archive: {error}"))?;
    if archive.format_version != 1 {
        return Err(format!(
            "Unsupported project package archive format version '{}'.",
            archive.format_version
        ));
    }

    for entry in archive.entries {
        validate_relative_archive_path(entry.path.as_str())?;
        let target_path = output_root.join(entry.path.as_str());
        match entry.kind.as_str() {
            "dir" => {
                fs::create_dir_all(&target_path).map_err(|error| {
                    format!(
                        "Failed to create restored directory '{}': {}",
                        target_path.display(),
                        error
                    )
                })?;
            }
            "file" => {
                let encoded = entry.contents_base64.ok_or_else(|| {
                    format!("Archive entry '{}' is missing file contents.", entry.path)
                })?;
                let decoded = BASE64_STANDARD
                    .decode(encoded.as_bytes())
                    .map_err(|error| {
                        format!(
                            "Failed to decode file contents for archive entry '{}': {}",
                            entry.path, error
                        )
                    })?;
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent).map_err(|error| {
                        format!(
                            "Failed to create restored parent directory '{}': {}",
                            parent.display(),
                            error
                        )
                    })?;
                }
                fs::write(&target_path, decoded).map_err(|error| {
                    format!(
                        "Failed to write restored file '{}': {}",
                        target_path.display(),
                        error
                    )
                })?;
            }
            other => {
                return Err(format!(
                    "Archive entry '{}' has unsupported kind '{}'.",
                    entry.path, other
                ));
            }
        }
    }

    Ok(())
}

pub(crate) fn directory_size_bytes(root: &Path) -> Result<u64, String> {
    let metadata = fs::metadata(root).map_err(|error| {
        format!(
            "Failed to read packaged path metadata '{}' while measuring size: {}",
            root.display(),
            error
        )
    })?;
    if metadata.is_file() {
        return Ok(metadata.len());
    }

    let mut total = 0_u64;
    let entries = fs::read_dir(root).map_err(|error| {
        format!(
            "Failed to read packaged directory '{}' while measuring size: {}",
            root.display(),
            error
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "Failed to read packaged directory entry under '{}' while measuring size: {}",
                root.display(),
                error
            )
        })?;
        total = total
            .checked_add(directory_size_bytes(entry.path().as_path())?)
            .ok_or_else(|| "Packaged directory size exceeded supported limits.".to_string())?;
    }
    Ok(total)
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;

    let bytes_f64 = bytes as f64;
    if bytes_f64 >= GIB {
        format!("{:.1} GiB", bytes_f64 / GIB)
    } else if bytes_f64 >= MIB {
        format!("{:.1} MiB", bytes_f64 / MIB)
    } else if bytes_f64 >= KIB {
        format!("{:.1} KiB", bytes_f64 / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn validate_relative_archive_path(raw_path: &str) -> Result<(), String> {
    if raw_path.trim().is_empty() {
        return Err("Archive entry path cannot be empty.".to_string());
    }
    let candidate = Path::new(raw_path);
    if candidate.is_absolute() {
        return Err(format!(
            "Archive entry path '{}' must be relative.",
            raw_path
        ));
    }
    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "Archive entry path '{}' cannot use parent traversal (`..`).",
            raw_path
        ));
    }
    Ok(())
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    let hash = digest.finalize();
    let mut rendered = String::with_capacity(hash.len() * 2);
    for byte in hash {
        use std::fmt::Write as _;
        let _ = write!(&mut rendered, "{byte:02x}");
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::{
        create_package_archive_bytes, extract_package_archive_bytes,
        relocate_pulled_package_receipt, sha256_hex, PackageArchiveDocument,
    };
    use base64::Engine as _;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(stem: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("cargo-ai-projects-command-test-{stem}-{nanos}"))
    }

    #[test]
    fn archive_round_trip_preserves_directory_structure() {
        let source_root = temp_dir("archive-source");
        let dest_root = temp_dir("archive-dest");
        fs::create_dir_all(source_root.join(".cargo-ai"))
            .expect("source metadata dir should be created");
        fs::create_dir_all(source_root.join("assets"))
            .expect("source assets dir should be created");
        fs::write(
            source_root.join(".cargo-ai/project.toml"),
            "format_version = 1\n",
        )
        .expect("project metadata should be written");
        fs::write(
            source_root.join("cargo-ai-package.toml"),
            "format_version = 1\n",
        )
        .expect("package manifest should be written");
        fs::write(source_root.join("assets/demo.txt"), "hello").expect("asset should be written");

        let archive_bytes =
            create_package_archive_bytes(source_root.as_path()).expect("archive should serialize");

        fs::create_dir_all(&dest_root).expect("dest root should be created");
        extract_package_archive_bytes(archive_bytes.as_slice(), dest_root.as_path())
            .expect("archive should restore");
        relocate_pulled_package_receipt(dest_root.as_path())
            .expect("receipt should move into pulled-project origin metadata");

        assert_eq!(
            fs::read_to_string(dest_root.join(".cargo-ai/project.toml"))
                .expect("restored project metadata should be readable"),
            "format_version = 1\n"
        );
        assert_eq!(
            fs::read_to_string(dest_root.join(".cargo-ai/origin/cargo-ai-package.toml"))
                .expect("restored receipt should be readable"),
            "format_version = 1\n"
        );
        assert!(
            !dest_root.join("cargo-ai-package.toml").exists(),
            "root-level package receipt should be moved into origin metadata"
        );
        assert_eq!(
            fs::read_to_string(dest_root.join("assets/demo.txt"))
                .expect("restored asset should be readable"),
            "hello"
        );

        let _ = fs::remove_dir_all(source_root);
        let _ = fs::remove_dir_all(dest_root);
    }

    #[test]
    fn sha256_hex_renders_expected_length() {
        let rendered = sha256_hex(b"hello");
        assert_eq!(rendered.len(), 64);
        assert_eq!(
            rendered,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn legacy_json_archive_still_restores_successfully() {
        let source_root = temp_dir("legacy-archive-source");
        let dest_root = temp_dir("legacy-archive-dest");
        fs::create_dir_all(source_root.join(".cargo-ai"))
            .expect("source metadata dir should be created");
        fs::write(
            source_root.join(".cargo-ai/project.toml"),
            "format_version = 1\n",
        )
        .expect("project metadata should be written");
        fs::write(
            source_root.join("cargo-ai-package.toml"),
            "format_version = 1\n",
        )
        .expect("package manifest should be written");
        let legacy_archive = PackageArchiveDocument {
            format_version: 1,
            entries: vec![
                super::PackageArchiveEntry {
                    path: ".cargo-ai".to_string(),
                    kind: "dir".to_string(),
                    contents_base64: None,
                },
                super::PackageArchiveEntry {
                    path: ".cargo-ai/project.toml".to_string(),
                    kind: "file".to_string(),
                    contents_base64: Some(
                        base64::engine::general_purpose::STANDARD
                            .encode("format_version = 1\n".as_bytes()),
                    ),
                },
                super::PackageArchiveEntry {
                    path: "cargo-ai-package.toml".to_string(),
                    kind: "file".to_string(),
                    contents_base64: Some(
                        base64::engine::general_purpose::STANDARD
                            .encode("format_version = 1\n".as_bytes()),
                    ),
                },
            ],
        };
        let archive_bytes =
            serde_json::to_vec(&legacy_archive).expect("legacy archive should serialize");

        fs::create_dir_all(&dest_root).expect("dest root should be created");
        extract_package_archive_bytes(archive_bytes.as_slice(), dest_root.as_path())
            .expect("legacy archive should restore");
        relocate_pulled_package_receipt(dest_root.as_path())
            .expect("receipt should move into pulled-project origin metadata");

        assert!(dest_root.join(".cargo-ai/project.toml").exists());
        assert!(dest_root
            .join(".cargo-ai/origin/cargo-ai-package.toml")
            .exists());

        let _ = fs::remove_dir_all(source_root);
        let _ = fs::remove_dir_all(dest_root);
    }
}
