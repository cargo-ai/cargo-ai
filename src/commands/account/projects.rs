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
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
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

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PulledPackageHostedReceiptDocument {
    format_version: u32,
    source_kind: String,
    hosted_source_id: String,
    hosted_version_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_handle: Option<String>,
    package_name: String,
    resolved_version: String,
    package_sha256: String,
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
const PULLED_PACKAGE_HOSTED_RECEIPT_RELATIVE_PATH: &str =
    ".cargo-ai/origin/cargo-ai-package-receipt.toml";
const ESTIMATED_PUBLISH_ACCESS_TOKEN: &str = "__publish-size-estimate__";
#[cfg(feature = "developer-tools")]
const SAFE_PROJECT_PUBLISH_REQUEST_LIMIT_BYTES: u64 = 5_500_000;
const HOSTED_ARCHIVE_MAX_COMPRESSED_BYTES: usize = 10 * 1024 * 1024;
const HOSTED_ARCHIVE_MAX_EXPANDED_BYTES: u64 = 100 * 1024 * 1024;
const HOSTED_ARCHIVE_MAX_ENTRIES: usize = 10_000;
const HOSTED_ARCHIVE_MAX_PATH_BYTES: usize = 1_024;
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

#[derive(Clone, Copy)]
struct ArchiveLimits {
    compressed_bytes: usize,
    expanded_bytes: u64,
    entries: usize,
    path_bytes: usize,
}

const HOSTED_ARCHIVE_LIMITS: ArchiveLimits = ArchiveLimits {
    compressed_bytes: HOSTED_ARCHIVE_MAX_COMPRESSED_BYTES,
    expanded_bytes: HOSTED_ARCHIVE_MAX_EXPANDED_BYTES,
    entries: HOSTED_ARCHIVE_MAX_ENTRIES,
    path_bytes: HOSTED_ARCHIVE_MAX_PATH_BYTES,
};

pub async fn run(projects_m: &ArgMatches) -> bool {
    let projects_command = if let Some(list_m) = projects_m.subcommand_matches("list") {
        let owner_handle =
            crate::commands::local_packages::account_handle_from_list_matches(list_m).flatten();
        ProjectsCommand::List {
            owner_handle,
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
            if let Err(error) =
                validate_project_pull_response_matches_request(&response, name, version.as_deref())
            {
                eprintln!("x {error}");
                return false;
            }
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

pub(crate) fn validate_project_pull_response_matches_request(
    response: &Value,
    requested_name: &str,
    requested_version: Option<&str>,
) -> Result<(), String> {
    let resolved_name = response
        .get("project")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Hosted pull response did not include `project`.".to_string())?;
    if resolved_name != requested_name {
        return Err(format!(
            "Hosted pull response returned package `{resolved_name}` for requested package `{requested_name}`."
        ));
    }

    let resolved_version_raw = response
        .get("project_version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Hosted pull response did not include `project_version`.".to_string())?;
    let resolved_version = Version::parse(resolved_version_raw).map_err(|error| {
        format!(
            "Hosted pull response returned invalid package version `{resolved_version_raw}`: {error}"
        )
    })?;
    if let Some(requested_version_raw) = requested_version {
        let requested_version = Version::parse(requested_version_raw).map_err(|error| {
            format!(
                "Requested hosted package version `{requested_version_raw}` is invalid: {error}"
            )
        })?;
        if resolved_version != requested_version {
            return Err(format!(
                "Hosted pull response returned version {resolved_version} for exact requested version {requested_version}."
            ));
        }
    }
    Ok(())
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
    let declared_size_bytes = usize::try_from(package_size_bytes).map_err(|_| {
        "Pull response declared an invalid negative or unsupported package archive size."
            .to_string()
    })?;
    validate_package_archive_size(declared_size_bytes)?;

    let archive_bytes = decode_package_archive_base64(archive_base64)?;
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
    validate_restored_package_manifest(response, output_path)?;
    relocate_pulled_package_receipt(output_path)?;
    write_pulled_package_hosted_receipt(response, output_path)
}

fn validate_restored_package_manifest(response: &Value, project_root: &Path) -> Result<(), String> {
    let manifest_path = project_root.join(PACKAGE_MANIFEST_FILE_NAME);
    let contents = fs::read_to_string(&manifest_path).map_err(|error| {
        format!(
            "Failed to read restored package manifest '{}': {}",
            manifest_path.display(),
            error
        )
    })?;
    let manifest: toml::Value = toml::from_str(contents.as_str()).map_err(|error| {
        format!(
            "Failed to parse restored package manifest '{}': {}",
            manifest_path.display(),
            error
        )
    })?;
    let manifest_name = manifest
        .get("project_name")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            format!(
                "Restored package manifest '{}' is missing `project_name`.",
                manifest_path.display()
            )
        })?;
    let manifest_version_raw = manifest
        .get("project_version")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            format!(
                "Restored package manifest '{}' is missing `project_version`.",
                manifest_path.display()
            )
        })?;
    let response_name = response
        .get("project")
        .and_then(Value::as_str)
        .ok_or_else(|| "Hosted pull response did not include `project`.".to_string())?;
    let response_version_raw = response
        .get("project_version")
        .and_then(Value::as_str)
        .ok_or_else(|| "Hosted pull response did not include `project_version`.".to_string())?;
    let manifest_version = Version::parse(manifest_version_raw).map_err(|error| {
        format!("Restored package manifest version `{manifest_version_raw}` is invalid: {error}")
    })?;
    let response_version = Version::parse(response_version_raw).map_err(|error| {
        format!("Hosted pull response version `{response_version_raw}` is invalid: {error}")
    })?;

    if manifest_name != response_name || manifest_version != response_version {
        return Err(format!(
            "Restored package manifest identity `{manifest_name}` {manifest_version} did not match hosted response identity `{response_name}` {response_version}."
        ));
    }
    Ok(())
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

fn write_pulled_package_hosted_receipt(
    response: &Value,
    project_root: &Path,
) -> Result<(), String> {
    let hosted_source_id = response
        .get("hosted_source_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "Pull succeeded but response did not include `hosted_source_id`.".to_string()
        })?;
    let hosted_version_id = response
        .get("hosted_version_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "Pull succeeded but response did not include `hosted_version_id`.".to_string()
        })?;
    let package_name = response
        .get("project")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Pull succeeded but response did not include `project`.".to_string())?;
    let resolved_version = response
        .get("project_version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "Pull succeeded but response did not include `project_version`.".to_string()
        })?;
    let package_sha256 = response
        .get("package_sha256")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "Pull succeeded but response did not include `package_sha256`.".to_string()
        })?;
    let receipt = PulledPackageHostedReceiptDocument {
        format_version: 1,
        source_kind: "hosted".to_string(),
        hosted_source_id: hosted_source_id.to_string(),
        hosted_version_id: hosted_version_id.to_string(),
        owner_handle: response
            .get("owner_handle")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
        package_name: package_name.to_string(),
        resolved_version: resolved_version.to_string(),
        package_sha256: package_sha256.to_string(),
    };
    let receipt_path = project_root.join(PULLED_PACKAGE_HOSTED_RECEIPT_RELATIVE_PATH);
    if let Some(parent) = receipt_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create pulled package hosted receipt directory '{}': {}",
                parent.display(),
                error
            )
        })?;
    }
    let rendered = toml::to_string_pretty(&receipt)
        .map_err(|error| format!("Failed to render pulled package hosted receipt: {error}"))?;
    fs::write(&receipt_path, rendered).map_err(|error| {
        format!(
            "Failed to write pulled package hosted receipt '{}': {}",
            receipt_path.display(),
            error
        )
    })
}

fn prepare_output_directory(path: &Path, force: bool) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if archive_metadata_is_link_like(&metadata) {
                return Err(format!(
                    "Output path '{}' must not be a symbolic link or reparse point.",
                    path.display()
                ));
            }
            if !force {
                return Err(format!(
                    "Output directory '{}' already exists. Re-run with --force to replace it, or choose --output-dir <DIR>.",
                    path.display()
                ));
            }

            if metadata.is_dir() {
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
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "Failed to inspect output path '{}': {}",
                path.display(),
                error
            ));
        }
    }

    fs::create_dir_all(path).map_err(|error| {
        format!(
            "Failed to create output directory '{}': {}",
            path.display(),
            error
        )
    })?;
    ensure_archive_path_is_safe(path, Path::new(""))
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

pub(crate) fn extract_package_archive_bytes(
    archive_bytes: &[u8],
    output_root: &Path,
) -> Result<(), String> {
    extract_package_archive_bytes_with_limits(archive_bytes, output_root, HOSTED_ARCHIVE_LIMITS)
}

pub(crate) fn validate_package_archive_size(compressed_bytes: usize) -> Result<(), String> {
    validate_archive_compressed_size(compressed_bytes, HOSTED_ARCHIVE_LIMITS)
}

pub(crate) fn decode_package_archive_base64(archive_base64: &str) -> Result<Vec<u8>, String> {
    validate_archive_base64_size(archive_base64.len(), HOSTED_ARCHIVE_LIMITS)?;
    let archive_bytes = BASE64_STANDARD
        .decode(archive_base64.as_bytes())
        .map_err(|error| format!("Failed to decode package archive: {error}"))?;
    validate_package_archive_size(archive_bytes.len())?;
    Ok(archive_bytes)
}

fn extract_package_archive_bytes_with_limits(
    archive_bytes: &[u8],
    output_root: &Path,
    limits: ArchiveLimits,
) -> Result<(), String> {
    validate_archive_compressed_size(archive_bytes.len(), limits)?;
    if archive_bytes.starts_with(&[0x1f, 0x8b]) {
        extract_compressed_package_archive_bytes(archive_bytes, output_root, limits)
    } else {
        extract_legacy_package_archive_bytes(archive_bytes, output_root, limits)
    }
}

fn extract_compressed_package_archive_bytes(
    archive_bytes: &[u8],
    output_root: &Path,
    limits: ArchiveLimits,
) -> Result<(), String> {
    let decoder = GzDecoder::new(Cursor::new(archive_bytes));
    let mut archive = Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| format!("Failed to read compressed project archive entries: {error}"))?;
    let mut entry_count = 0_usize;
    let mut expanded_bytes = 0_u64;

    for entry in entries {
        let mut entry = entry
            .map_err(|error| format!("Failed to read compressed project archive entry: {error}"))?;
        let entry_path = entry.path().map_err(|error| {
            format!("Failed to read compressed project archive entry path: {error}")
        })?;
        let relative_path = entry_path.to_string_lossy().replace('\\', "/");
        let relative_path = validate_relative_archive_path(relative_path.as_str())?;
        let relative_path_text = relative_path.to_string_lossy().replace('\\', "/");
        let entry_size = match entry.header().entry_type() {
            EntryType::Directory => 0,
            EntryType::Regular => entry.size(),
            other => {
                return Err(format!(
                    "Compressed archive entry '{}' has unsupported kind '{:?}'.",
                    relative_path_text, other
                ));
            }
        };
        update_archive_budget(
            &mut entry_count,
            &mut expanded_bytes,
            relative_path_text.as_str(),
            entry_size,
            limits,
        )?;
        let target_path = resolve_archive_target_path(output_root, relative_path.as_path())?;
        match entry.header().entry_type() {
            EntryType::Directory => {
                fs::create_dir_all(&target_path).map_err(|error| {
                    format!(
                        "Failed to create restored directory '{}': {}",
                        target_path.display(),
                        error
                    )
                })?;
                ensure_archive_target_is_safe(output_root, relative_path.as_path())?;
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
                ensure_archive_target_parent_is_safe(output_root, relative_path.as_path())?;
                entry.unpack(&target_path).map_err(|error| {
                    format!(
                        "Failed to write restored file '{}': {}",
                        target_path.display(),
                        error
                    )
                })?;
                ensure_archive_target_is_safe(output_root, relative_path.as_path())?;
            }
            _ => unreachable!("unsupported archive entry kinds rejected before extraction"),
        }
    }

    Ok(())
}

fn extract_legacy_package_archive_bytes(
    archive_bytes: &[u8],
    output_root: &Path,
    limits: ArchiveLimits,
) -> Result<(), String> {
    let archive: PackageArchiveDocument = serde_json::from_slice(archive_bytes)
        .map_err(|error| format!("Failed to parse project package archive: {error}"))?;
    if archive.format_version != 1 {
        return Err(format!(
            "Unsupported project package archive format version '{}'.",
            archive.format_version
        ));
    }

    let mut entry_count = 0_usize;
    let mut expanded_bytes = 0_u64;
    for entry in archive.entries {
        let relative_path = validate_relative_archive_path(entry.path.as_str())?;
        let relative_path_text = relative_path.to_string_lossy().replace('\\', "/");
        match entry.kind.as_str() {
            "dir" => {
                update_archive_budget(
                    &mut entry_count,
                    &mut expanded_bytes,
                    relative_path_text.as_str(),
                    0,
                    limits,
                )?;
                let target_path =
                    resolve_archive_target_path(output_root, relative_path.as_path())?;
                fs::create_dir_all(&target_path).map_err(|error| {
                    format!(
                        "Failed to create restored directory '{}': {}",
                        target_path.display(),
                        error
                    )
                })?;
                ensure_archive_target_is_safe(output_root, relative_path.as_path())?;
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
                update_archive_budget(
                    &mut entry_count,
                    &mut expanded_bytes,
                    relative_path_text.as_str(),
                    u64::try_from(decoded.len()).map_err(|_| {
                        "Legacy archive entry exceeded supported size limits.".to_string()
                    })?,
                    limits,
                )?;
                let target_path =
                    resolve_archive_target_path(output_root, relative_path.as_path())?;
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent).map_err(|error| {
                        format!(
                            "Failed to create restored parent directory '{}': {}",
                            parent.display(),
                            error
                        )
                    })?;
                }
                ensure_archive_target_parent_is_safe(output_root, relative_path.as_path())?;
                fs::write(&target_path, decoded).map_err(|error| {
                    format!(
                        "Failed to write restored file '{}': {}",
                        target_path.display(),
                        error
                    )
                })?;
                ensure_archive_target_is_safe(output_root, relative_path.as_path())?;
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

fn validate_archive_compressed_size(
    compressed_bytes: usize,
    limits: ArchiveLimits,
) -> Result<(), String> {
    if compressed_bytes > limits.compressed_bytes {
        return Err(format!(
            "Package archive is {} bytes after decoding; the client limit is {} bytes.",
            compressed_bytes, limits.compressed_bytes
        ));
    }
    Ok(())
}

fn validate_archive_base64_size(encoded_bytes: usize, limits: ArchiveLimits) -> Result<(), String> {
    let max_encoded_bytes = limits
        .compressed_bytes
        .checked_add(2)
        .and_then(|value| value.checked_div(3))
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| "Package archive size limit overflowed supported bounds.".to_string())?;
    if encoded_bytes > max_encoded_bytes {
        return Err(format!(
            "Package archive base64 payload exceeds the {}-byte client limit.",
            max_encoded_bytes
        ));
    }
    Ok(())
}

fn update_archive_budget(
    entry_count: &mut usize,
    expanded_bytes: &mut u64,
    relative_path: &str,
    entry_size: u64,
    limits: ArchiveLimits,
) -> Result<(), String> {
    if relative_path.len() > limits.path_bytes {
        return Err(format!(
            "Archive entry path exceeds the {}-byte client limit.",
            limits.path_bytes
        ));
    }
    *entry_count = entry_count
        .checked_add(1)
        .ok_or_else(|| "Archive entry count exceeded supported limits.".to_string())?;
    if *entry_count > limits.entries {
        return Err(format!(
            "Package archive contains more than {} entries.",
            limits.entries
        ));
    }
    *expanded_bytes = expanded_bytes
        .checked_add(entry_size)
        .ok_or_else(|| "Expanded package archive size exceeded supported limits.".to_string())?;
    if *expanded_bytes > limits.expanded_bytes {
        return Err(format!(
            "Expanded package archive exceeds the {}-byte client limit.",
            limits.expanded_bytes
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn archive_metadata_is_link_like(metadata: &fs::Metadata) -> bool {
    archive_windows_attributes_are_link_like(metadata.file_attributes())
}

#[cfg(windows)]
fn archive_windows_attributes_are_link_like(attributes: u32) -> bool {
    attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn archive_metadata_is_link_like(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn resolve_archive_target_path(
    output_root: &Path,
    relative_path: &Path,
) -> Result<PathBuf, String> {
    ensure_archive_target_parent_is_safe(output_root, relative_path)?;
    Ok(output_root.join(relative_path))
}

fn ensure_archive_target_parent_is_safe(
    output_root: &Path,
    relative_path: &Path,
) -> Result<(), String> {
    let parent = relative_path.parent().unwrap_or_else(|| Path::new(""));
    ensure_archive_path_is_safe(output_root, parent)
}

fn ensure_archive_target_is_safe(output_root: &Path, relative_path: &Path) -> Result<(), String> {
    ensure_archive_path_is_safe(output_root, relative_path)
}

fn ensure_archive_path_is_safe(output_root: &Path, relative_path: &Path) -> Result<(), String> {
    let root_metadata = fs::symlink_metadata(output_root).map_err(|error| {
        format!(
            "Failed to inspect archive output root '{}': {}",
            output_root.display(),
            error
        )
    })?;
    if archive_metadata_is_link_like(&root_metadata) || !root_metadata.is_dir() {
        return Err(format!(
            "Archive output root '{}' must be a real directory and not a symbolic link or reparse point.",
            output_root.display()
        ));
    }

    let mut current = output_root.to_path_buf();
    for component in relative_path.components() {
        match component {
            Component::CurDir => continue,
            Component::Normal(segment) => current.push(segment),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("Archive target path escaped the output root.".to_string());
            }
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if archive_metadata_is_link_like(&metadata) => {
                return Err(format!(
                    "Archive target path must not traverse symbolic link or reparse point '{}'.",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(format!(
                    "Failed to inspect archive target component '{}': {}",
                    current.display(),
                    error
                ));
            }
        }
    }
    Ok(())
}

fn validate_relative_archive_path(raw_path: &str) -> Result<PathBuf, String> {
    crate::commands::local_packages::normalize_portable_relative_path(
        raw_path,
        "Archive entry path",
    )
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
        create_package_archive_bytes, extract_compressed_package_archive_bytes,
        extract_legacy_package_archive_bytes, extract_package_archive_bytes,
        extract_package_archive_bytes_with_limits, relocate_pulled_package_receipt, sha256_hex,
        validate_archive_base64_size, validate_project_pull_response_matches_request,
        validate_relative_archive_path, validate_restored_package_manifest, ArchiveLimits,
        PackageArchiveDocument, PackageArchiveEntry, PulledPackageHostedReceiptDocument,
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

    #[test]
    fn compressed_archive_limits_entry_count_and_expanded_bytes() {
        let source_root = temp_dir("compressed-limits-source");
        let dest_root = temp_dir("compressed-limits-dest");
        fs::create_dir_all(&source_root).expect("source root should be created");
        fs::create_dir_all(&dest_root).expect("dest root should be created");
        fs::write(source_root.join("one.txt"), "1234").expect("first file should be written");
        fs::write(source_root.join("two.txt"), "5678").expect("second file should be written");
        let archive =
            create_package_archive_bytes(source_root.as_path()).expect("archive should serialize");

        let entry_error = extract_compressed_package_archive_bytes(
            archive.as_slice(),
            dest_root.as_path(),
            ArchiveLimits {
                compressed_bytes: archive.len(),
                expanded_bytes: 100,
                entries: 1,
                path_bytes: 1_024,
            },
        )
        .expect_err("compressed entry count should be limited");
        assert!(entry_error.contains("more than 1 entries"));

        let expanded_dest = temp_dir("compressed-expanded-dest");
        fs::create_dir_all(&expanded_dest).expect("expanded dest should be created");
        let expanded_error = extract_compressed_package_archive_bytes(
            archive.as_slice(),
            expanded_dest.as_path(),
            ArchiveLimits {
                compressed_bytes: archive.len(),
                expanded_bytes: 3,
                entries: 10,
                path_bytes: 1_024,
            },
        )
        .expect_err("compressed expanded bytes should be limited");
        assert!(expanded_error.contains("Expanded package archive exceeds"));

        let _ = fs::remove_dir_all(source_root);
        let _ = fs::remove_dir_all(dest_root);
        let _ = fs::remove_dir_all(expanded_dest);
    }

    #[test]
    fn legacy_archive_limits_entry_count_and_expanded_bytes() {
        let archive = PackageArchiveDocument {
            format_version: 1,
            entries: vec![
                PackageArchiveEntry {
                    path: "one.txt".to_string(),
                    kind: "file".to_string(),
                    contents_base64: Some(
                        base64::engine::general_purpose::STANDARD.encode(b"1234"),
                    ),
                },
                PackageArchiveEntry {
                    path: "two.txt".to_string(),
                    kind: "file".to_string(),
                    contents_base64: Some(
                        base64::engine::general_purpose::STANDARD.encode(b"5678"),
                    ),
                },
            ],
        };
        let archive = serde_json::to_vec(&archive).expect("legacy archive should serialize");
        let entry_dest = temp_dir("legacy-entry-limit");
        fs::create_dir_all(&entry_dest).expect("entry dest should be created");
        let entry_error = extract_legacy_package_archive_bytes(
            archive.as_slice(),
            entry_dest.as_path(),
            ArchiveLimits {
                compressed_bytes: archive.len(),
                expanded_bytes: 100,
                entries: 1,
                path_bytes: 1_024,
            },
        )
        .expect_err("legacy entry count should be limited");
        assert!(entry_error.contains("more than 1 entries"));

        let expanded_dest = temp_dir("legacy-expanded-limit");
        fs::create_dir_all(&expanded_dest).expect("expanded dest should be created");
        let expanded_error = extract_legacy_package_archive_bytes(
            archive.as_slice(),
            expanded_dest.as_path(),
            ArchiveLimits {
                compressed_bytes: archive.len(),
                expanded_bytes: 3,
                entries: 10,
                path_bytes: 1_024,
            },
        )
        .expect_err("legacy expanded bytes should be limited");
        assert!(expanded_error.contains("Expanded package archive exceeds"));

        let _ = fs::remove_dir_all(entry_dest);
        let _ = fs::remove_dir_all(expanded_dest);
    }

    #[test]
    fn archive_paths_reject_windows_prefixes_on_every_platform() {
        for candidate in [
            "C:relative.txt",
            "C:\\absolute.txt",
            "\\\\server\\share\\file.txt",
            "\\\\?\\C:\\file.txt",
        ] {
            let error = validate_relative_archive_path(candidate)
                .expect_err("Windows-prefixed path should be rejected");
            assert!(
                error.contains("drive-relative")
                    || error.contains("device-root")
                    || error.contains("prefix"),
                "unexpected error for {candidate}: {error}"
            );
        }
    }

    #[test]
    fn archive_limits_decoded_size_and_normalized_path_length() {
        let error = extract_package_archive_bytes_with_limits(
            b"12345",
            PathBuf::from("unused").as_path(),
            ArchiveLimits {
                compressed_bytes: 4,
                expanded_bytes: 100,
                entries: 10,
                path_bytes: 1_024,
            },
        )
        .expect_err("decoded archive size should be checked before extraction");
        assert!(error.contains("after decoding"));

        let long_path = format!("{}.txt", "a".repeat(1_025));
        let error = validate_relative_archive_path(long_path.as_str())
            .expect_err("overlong normalized path should be rejected");
        assert!(error.contains("1024-byte path limit"));
    }

    #[test]
    fn archive_base64_size_is_bounded_before_decode() {
        let limits = ArchiveLimits {
            compressed_bytes: 4,
            expanded_bytes: 100,
            entries: 10,
            path_bytes: 1_024,
        };
        validate_archive_base64_size(8, limits).expect("base64 for four decoded bytes should fit");
        let error = validate_archive_base64_size(9, limits)
            .expect_err("oversized base64 should be rejected before decode");
        assert!(error.contains("base64 payload exceeds"));
    }

    #[test]
    fn hosted_pull_response_must_match_requested_identity_and_version() {
        let response = serde_json::json!({
            "project": "demo",
            "project_version": "1.2.3"
        });
        validate_project_pull_response_matches_request(&response, "demo", Some("1.2.3"))
            .expect("matching hosted response should pass");

        let package_error =
            validate_project_pull_response_matches_request(&response, "other", None)
                .expect_err("wrong package identity should fail");
        assert!(package_error.contains("requested package `other`"));
        let version_error =
            validate_project_pull_response_matches_request(&response, "demo", Some("1.2.2"))
                .expect_err("wrong exact version should fail");
        assert!(version_error.contains("exact requested version 1.2.2"));
    }

    #[test]
    fn restored_manifest_must_match_hosted_response_identity() {
        let root = temp_dir("restored-manifest-identity");
        fs::create_dir_all(&root).expect("restored root should be writable");
        fs::write(
            root.join("cargo-ai-package.toml"),
            r#"format_version = 1
project_name = "demo"
project_version = "1.2.3"
"#,
        )
        .expect("restored manifest should be writable");
        let matching = serde_json::json!({
            "project": "demo",
            "project_version": "1.2.3"
        });
        validate_restored_package_manifest(&matching, &root)
            .expect("matching restored manifest should pass");

        let mismatched = serde_json::json!({
            "project": "other",
            "project_version": "1.2.3"
        });
        let error = validate_restored_package_manifest(&mismatched, &root)
            .expect_err("mismatched restored manifest should fail");
        assert!(error.contains("did not match hosted response identity"));

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn pull_output_rejects_symlink_root_without_touching_target() {
        use super::prepare_output_directory;
        use std::os::unix::fs::symlink;

        let root = temp_dir("pull-output-symlink");
        let external = root.join("external");
        let output = root.join("output");
        fs::create_dir_all(&external).expect("external output should be writable");
        fs::write(external.join("sentinel.txt"), "outside")
            .expect("external sentinel should be writable");
        symlink(&external, &output).expect("output symlink should be created");

        let error = prepare_output_directory(&output, true)
            .expect_err("symlinked output root should be rejected");
        assert!(error.contains("symbolic link"));
        assert_eq!(
            fs::read_to_string(external.join("sentinel.txt"))
                .expect("external sentinel should remain"),
            "outside"
        );

        let _ = fs::remove_file(output);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn hosted_pull_receipt_reads_legacy_owner_id_but_omits_new_emission() {
        let legacy = r#"
format_version = 1
source_kind = "hosted"
hosted_source_id = "source-id"
hosted_version_id = "version-id"
owner_account_id = "private-account-id"
owner_handle = "alice"
package_name = "demo"
resolved_version = "1.0.0"
package_sha256 = "abc"
"#;
        let receipt: PulledPackageHostedReceiptDocument =
            toml::from_str(legacy).expect("legacy hosted receipt should remain readable");
        assert_eq!(receipt.hosted_source_id, "source-id");

        let rendered = toml::to_string_pretty(&receipt).expect("receipt should serialize");
        assert!(!rendered.contains("owner_account_id"));
    }

    #[cfg(windows)]
    #[test]
    fn archive_output_rejects_windows_reparse_attributes() {
        assert!(super::archive_windows_attributes_are_link_like(
            super::FILE_ATTRIBUTE_REPARSE_POINT
        ));
        assert!(!super::archive_windows_attributes_are_link_like(0));
    }
}
