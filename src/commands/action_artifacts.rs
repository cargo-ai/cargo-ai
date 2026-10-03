//! Bounded private artifact identity, authorization and passive reads.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::PathBuf};

#[path = "action_artifacts_io.rs"]
mod confined_io;
pub(crate) use crate::business_schema::ArtifactNomination as Nomination;
pub(crate) const MAX_ARTIFACT_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub(crate) const SUPPORTED_TYPES: &[&str] =
    &["text/plain", "application/json", "image/png", "audio/wav"];
const MAX_ITEMS: usize = 64;
const MAX_CHUNKS: usize = 4096;

/// Reads caller-selected ownership metadata with the same native confinement as artifacts.
/// The root must already identify the intended ownership boundary; no path is created.
pub(crate) fn read_owned_bytes(
    root: &std::path::Path,
    relative_path: &str,
    limit: usize,
) -> Result<Vec<u8>> {
    relative(relative_path)?;
    if limit > MAX_ARTIFACT_BYTES {
        return Err(too_large());
    }
    confined_io::read(root, relative_path, limit)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArtifactScope {
    pub id: String,
    pub path: String,
    pub mime_types: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Permission {
    pub version: u32,
    pub scopes: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Descriptor {
    pub schema_version: u32,
    pub id: String,
    pub reference: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub content_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadGrant {
    pub schema_version: u32,
    pub reference: String,
    pub interface: String,
    pub binding: Value,
    pub data_context_sha256: String,
    pub scope: String,
    pub relative_path: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub content_sha256: String,
}
/// The caller verifies ownership/configuration and retains any installed-data lease.
#[derive(Clone, Debug)]
pub(crate) struct Context {
    pub data_root: PathBuf,
    pub data_context_sha256: String,
    pub interface: String,
    pub binding: Value,
    pub scopes: Vec<ArtifactScope>,
    pub allowed_scopes: Vec<String>,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct ArtifactError {
    pub code: &'static str,
    pub message: &'static str,
}
impl std::fmt::Display for ArtifactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ArtifactError {}
type Result<T> = std::result::Result<T, ArtifactError>;
fn error(code: &'static str, message: &'static str) -> ArtifactError {
    ArtifactError { code, message }
}
fn denied() -> ArtifactError {
    error(
        "artifact.access_denied",
        "Artifact access is not authorized.",
    )
}
fn invalid() -> ArtifactError {
    error(
        "artifact.invalid_request",
        "The artifact request is invalid.",
    )
}
fn unsafe_file() -> ArtifactError {
    error(
        "artifact.unsafe_file",
        "Artifact file confinement could not be established.",
    )
}
fn changed() -> ArtifactError {
    error(
        "artifact.content_changed",
        "The artifact changed; obtain a fresh reference.",
    )
}
fn unsupported() -> ArtifactError {
    error(
        "artifact.unsupported_type",
        "The artifact type or format is unsupported.",
    )
}
fn too_large() -> ArtifactError {
    error(
        "artifact.size_limit",
        "The artifact exceeds its bounded delivery limit.",
    )
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        && !matches!(value, "__proto__" | "constructor" | "prototype")
}
fn relative(value: &str) -> Result<()> {
    if value.len() > 1024
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, '*' | '?' | '<' | '>' | '|' | '"'))
        || value.contains('\\')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(invalid());
    }
    super::runtime_data::portable_relative_path(std::path::Path::new(value), "Artifact")
        .map(|_| ())
        .map_err(|_| invalid())
}
fn unique_ids(ids: &[String]) -> bool {
    ids.len() <= MAX_ITEMS
        && ids.iter().all(|id| identifier(id))
        && ids.iter().collect::<BTreeSet<_>>().len() == ids.len()
}
pub(crate) fn validate_scopes(scopes: &[ArtifactScope]) -> Result<()> {
    if scopes.len() > MAX_ITEMS {
        return Err(invalid());
    }
    let mut ids = BTreeSet::new();
    for scope in scopes {
        if !identifier(&scope.id) || !ids.insert(&scope.id) {
            return Err(invalid());
        }
        relative(&scope.path)?;
        if scope.mime_types.is_empty()
            || scope.mime_types.len() > SUPPORTED_TYPES.len()
            || scope.mime_types.iter().collect::<BTreeSet<_>>().len() != scope.mime_types.len()
            || scope
                .mime_types
                .iter()
                .any(|mime| !SUPPORTED_TYPES.contains(&mime.as_str()))
        {
            return Err(unsupported());
        }
    }
    Ok(())
}
/// Every producer scope requires explicit native permission before any action runs.
pub(crate) fn validate_access(
    scopes: &[ArtifactScope],
    interface_ids: &[String],
    producer_ids: &[String],
    permission: Option<&Permission>,
) -> Result<Vec<String>> {
    validate_scopes(scopes)?;
    if !unique_ids(interface_ids)
        || !unique_ids(producer_ids)
        || interface_ids
            .iter()
            .any(|id| !scopes.iter().any(|scope| &scope.id == id))
        || producer_ids.iter().any(|id| !interface_ids.contains(id))
    {
        return Err(denied());
    }
    let granted = match permission {
        Some(permission)
            if permission.version == 1
                && unique_ids(&permission.scopes)
                && permission
                    .scopes
                    .iter()
                    .all(|id| interface_ids.contains(id)) =>
        {
            permission.scopes.as_slice()
        }
        Some(_) => return Err(denied()),
        None => &[],
    };
    if producer_ids.iter().any(|id| !granted.contains(id)) {
        return Err(denied());
    }
    Ok(producer_ids.to_vec())
}
fn selected_scope<'a>(context: &'a Context, id: &str, mime: &str) -> Result<&'a ArtifactScope> {
    validate_scopes(&context.scopes)?;
    if !identifier(&context.interface)
        || !valid_digest(&context.data_context_sha256)
        || !context.binding.is_object()
    {
        return Err(invalid());
    }
    if !context.allowed_scopes.iter().any(|allowed| allowed == id) {
        return Err(denied());
    }
    let scope = context
        .scopes
        .iter()
        .find(|scope| scope.id == id)
        .ok_or_else(denied)?;
    if !scope.mime_types.iter().any(|allowed| allowed == mime) {
        return Err(unsupported());
    }
    Ok(scope)
}
/// Canonical object-key ordering makes identity independent of JSON member order.
fn canonical(value: &Value, bytes: &mut Vec<u8>) {
    match value {
        Value::Object(map) => {
            bytes.push(b'{');
            let sorted: std::collections::BTreeMap<_, _> = map.iter().collect();
            for (index, (key, value)) in sorted.into_iter().enumerate() {
                if index != 0 {
                    bytes.push(b',');
                }
                bytes.extend(serde_json::to_vec(key).expect("JSON string serialization"));
                bytes.push(b':');
                canonical(value, bytes);
            }
            bytes.push(b'}');
        }
        Value::Array(values) => {
            bytes.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    bytes.push(b',');
                }
                canonical(value, bytes);
            }
            bytes.push(b']');
        }
        _ => bytes.extend(serde_json::to_vec(value).expect("JSON value serialization")),
    }
}
fn reference(grant: &ReadGrant) -> String {
    let mut fields = serde_json::to_value(grant).expect("Grant serialization");
    fields
        .as_object_mut()
        .expect("Grant object")
        .remove("reference");
    let mut bytes = b"cargo-ai.artifact-read-grant.v1\0".to_vec();
    canonical(&fields, &mut bytes);
    digest(&bytes)
}
/// No partial vector escapes when any member fails verification.
pub(crate) fn issue(
    context: &Context,
    nominations: &[Nomination],
) -> Result<(Vec<Descriptor>, Vec<ReadGrant>)> {
    if nominations.len() > MAX_ITEMS {
        return Err(too_large());
    }
    let mut ids = BTreeSet::new();
    let mut descriptors = Vec::new();
    let mut grants = Vec::new();
    for item in nominations {
        if !identifier(&item.id) || !ids.insert(&item.id) {
            return Err(invalid());
        }
        relative(&item.path)?;
        let scope = selected_scope(context, &item.scope, &item.mime_type)?;
        let bytes = confined_io::read(
            &context.data_root,
            &format!("{}/{}", scope.path, item.path),
            MAX_ARTIFACT_BYTES,
        )?;
        validate_format(&item.mime_type, &bytes)?;
        let mut grant = ReadGrant {
            schema_version: 1,
            reference: String::new(),
            interface: context.interface.clone(),
            binding: context.binding.clone(),
            data_context_sha256: context.data_context_sha256.clone(),
            scope: item.scope.clone(),
            relative_path: item.path.clone(),
            mime_type: item.mime_type.clone(),
            size_bytes: bytes.len() as u64,
            content_sha256: digest(&bytes),
        };
        grant.reference = reference(&grant);
        descriptors.push(Descriptor {
            schema_version: 1,
            id: item.id.clone(),
            reference: grant.reference.clone(),
            mime_type: grant.mime_type.clone(),
            size_bytes: grant.size_bytes,
            content_sha256: grant.content_sha256.clone(),
        });
        grants.push(grant);
    }
    Ok((descriptors, grants))
}
pub(crate) fn read(
    context: &Context,
    grant: &ReadGrant,
    requested_reference: &str,
) -> Result<Value> {
    if grant.schema_version != 1
        || requested_reference.len() > 128
        || !valid_digest(&grant.reference)
        || requested_reference != grant.reference
        || grant.reference != reference(grant)
        || grant.interface != context.interface
        || !valid_digest(&grant.content_sha256)
    {
        return Err(invalid());
    }
    if grant.binding != context.binding {
        return Err(error(
            "artifact.stale_binding",
            "The artifact catalog binding changed.",
        ));
    }
    if grant.data_context_sha256 != context.data_context_sha256 {
        return Err(error(
            "artifact.stale_context",
            "The artifact data context changed.",
        ));
    }
    if grant.size_bytes > MAX_ARTIFACT_BYTES as u64 {
        return Err(too_large());
    }
    relative(&grant.relative_path)?;
    let scope = selected_scope(context, &grant.scope, &grant.mime_type)?;
    let bytes = confined_io::read(
        &context.data_root,
        &format!("{}/{}", scope.path, grant.relative_path),
        MAX_ARTIFACT_BYTES,
    )?;
    if bytes.len() as u64 != grant.size_bytes || digest(&bytes) != grant.content_sha256 {
        return Err(changed());
    }
    validate_format(&grant.mime_type, &bytes)?;
    Ok(
        json!({"schema_version":1,"binding":context.binding,"interface":context.interface,"reference":grant.reference,"mime_type":grant.mime_type,"size_bytes":grant.size_bytes,"content_sha256":grant.content_sha256,"encoding":"base64","data":STANDARD.encode(bytes)}),
    )
}
fn validate_format(mime: &str, bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(too_large());
    }
    match mime {
        "text/plain" => {
            std::str::from_utf8(bytes).map_err(|_| unsupported())?;
            Ok(())
        }
        "application/json" => validate_json(bytes),
        "image/png" => validate_png(bytes),
        "audio/wav" => validate_wav(bytes),
        _ => Err(unsupported()),
    }
}

fn validate_json(bytes: &[u8]) -> Result<()> {
    crate::business_schema::strict_json_bounded(bytes, MAX_ARTIFACT_BYTES, 32)
        .map(|_| ())
        .map_err(|_| unsupported())
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}
fn validate_png(bytes: &[u8]) -> Result<()> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(unsupported());
    }
    let mut at = 8;
    let mut count = 0;
    let mut image_data = false;
    let mut ended_data = false;
    let mut color = 0;
    let mut bit_depth = 0;
    let mut palette = false;
    while at < bytes.len() {
        count += 1;
        if count > MAX_CHUNKS || bytes.len() - at < 12 {
            return Err(unsupported());
        }
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let end = at
            .checked_add(12)
            .and_then(|v| v.checked_add(len))
            .filter(|v| *v <= bytes.len())
            .ok_or_else(unsupported)?;
        let kind = &bytes[at + 4..at + 8];
        if !kind.iter().all(u8::is_ascii_alphabetic)
            || !kind[2].is_ascii_uppercase()
            || crc32(&bytes[at + 4..end - 4])
                != u32::from_be_bytes(bytes[end - 4..end].try_into().unwrap())
        {
            return Err(unsupported());
        }
        if count == 1 {
            if kind != b"IHDR" || len != 13 {
                return Err(unsupported());
            }
            let header = &bytes[at + 8..end - 4];
            let width = u32::from_be_bytes(header[..4].try_into().unwrap());
            let height = u32::from_be_bytes(header[4..8].try_into().unwrap());
            color = header[9];
            bit_depth = header[8];
            let valid_depth = match header[9] {
                0 => [1, 2, 4, 8, 16].contains(&header[8]),
                2 | 4 | 6 => [8, 16].contains(&header[8]),
                3 => [1, 2, 4, 8].contains(&header[8]),
                _ => false,
            };
            if width == 0
                || height == 0
                || width > 0x7fffffff
                || height > 0x7fffffff
                || !valid_depth
                || header[10] != 0
                || header[11] != 0
                || header[12] > 1
            {
                return Err(unsupported());
            }
        } else if kind == b"IHDR" {
            return Err(unsupported());
        }
        if kind == b"PLTE" {
            if palette
                || image_data
                || len == 0
                || len > 768
                || len % 3 != 0
                || matches!(color, 0 | 4)
                || (color == 3 && len / 3 > 1usize << bit_depth)
            {
                return Err(unsupported());
            }
            palette = true;
        }
        if kind == b"IDAT" {
            if ended_data || (color == 3 && !palette) {
                return Err(unsupported());
            }
            image_data = true;
        } else if image_data {
            ended_data = true;
        }
        if kind == b"IEND" {
            return if len == 0 && image_data && end == bytes.len() {
                Ok(())
            } else {
                Err(unsupported())
            };
        }
        if kind[0].is_ascii_uppercase()
            && ![b"IHDR".as_slice(), b"PLTE", b"IDAT", b"IEND"].contains(&kind)
        {
            return Err(unsupported());
        }
        at = end;
    }
    Err(unsupported())
}
fn validate_wav(bytes: &[u8]) -> Result<()> {
    if bytes.len() < 12
        || &bytes[..4] != b"RIFF"
        || &bytes[8..12] != b"WAVE"
        || u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize != bytes.len() - 8
    {
        return Err(unsupported());
    }
    let mut at = 12;
    let mut count = 0;
    let mut format = false;
    let mut data = false;
    while at < bytes.len() {
        count += 1;
        if count > MAX_CHUNKS || bytes.len() - at < 8 {
            return Err(unsupported());
        }
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let end = at
            .checked_add(8)
            .and_then(|v| v.checked_add(len))
            .filter(|v| *v <= bytes.len())
            .ok_or_else(unsupported)?;
        match &bytes[at..at + 4] {
            b"fmt " => {
                if format || data || len < 16 {
                    return Err(unsupported());
                }
                let head = &bytes[at + 8..end];
                if u16::from_le_bytes(head[..2].try_into().unwrap()) == 0
                    || u16::from_le_bytes(head[2..4].try_into().unwrap()) == 0
                    || u32::from_le_bytes(head[4..8].try_into().unwrap()) == 0
                    || u16::from_le_bytes(head[12..14].try_into().unwrap()) == 0
                {
                    return Err(unsupported());
                }
                format = true;
            }
            b"data" => {
                if !format || data {
                    return Err(unsupported());
                }
                data = true;
            }
            _ => {}
        }
        at = end
            .checked_add(len & 1)
            .filter(|v| *v <= bytes.len())
            .ok_or_else(unsupported)?;
    }
    if format && data && at == bytes.len() {
        Ok(())
    } else {
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        context: Context,
    }
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("cargo-artifacts-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(root.join("exports")).unwrap();
            Self {
                context: Context {
                    data_root: std::fs::canonicalize(root).unwrap(),
                    data_context_sha256: "a".repeat(64),
                    interface: "workspace".into(),
                    binding: json!({"root_sha256":"b".repeat(64),"catalog_sha256":"c".repeat(64),"content_sha256":"d".repeat(64)}),
                    scopes: vec![ArtifactScope {
                        id: "reports".into(),
                        path: "exports".into(),
                        mime_types: SUPPORTED_TYPES.iter().map(|s| s.to_string()).collect(),
                    }],
                    allowed_scopes: vec!["reports".into()],
                },
            }
        }
        fn put(&self, path: &str, bytes: &[u8], mime: &str) -> Nomination {
            std::fs::write(self.context.data_root.join("exports").join(path), bytes).unwrap();
            Nomination {
                id: "preview".into(),
                scope: "reports".into(),
                path: path.into(),
                mime_type: mime.into(),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.context.data_root).unwrap();
        }
    }
    fn png() -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        let header = [0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0];
        for (kind, data) in [
            (b"IHDR", header.as_slice()),
            (b"IDAT", &[120, 156, 99, 96, 96, 96, 0, 0, 0, 4, 0, 1][..]),
            (b"IEND", &[][..]),
        ] {
            bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let start = bytes.len();
            bytes.extend_from_slice(kind);
            bytes.extend_from_slice(data);
            bytes.extend_from_slice(&crc32(&bytes[start..]).to_be_bytes());
        }
        bytes
    }
    fn wav() -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&[1, 0, 1, 0]);
        bytes.extend_from_slice(&8000u32.to_le_bytes());
        bytes.extend_from_slice(&16000u32.to_le_bytes());
        bytes.extend_from_slice(&[2, 0, 16, 0]);
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes
    }
    #[test]
    fn all_four_types_issue_and_read_exact_existing_bytes() {
        let fixture = Fixture::new();
        for (mime, bytes) in [
            ("text/plain", b"<script>inert text</script>\n".to_vec()),
            (
                "application/json",
                br#"{"counter":18446744073709551615,"config":null}"#.to_vec(),
            ),
            ("image/png", png()),
            ("audio/wav", wav()),
        ] {
            let nomination = fixture.put("artifact", &bytes, mime);
            let (descriptors, grants) = issue(&fixture.context, &[nomination]).unwrap();
            let value = read(&fixture.context, &grants[0], &descriptors[0].reference).unwrap();
            assert_eq!(
                STANDARD.decode(value["data"].as_str().unwrap()).unwrap(),
                bytes
            );
            assert_eq!(value["content_sha256"], digest(&bytes));
            let page = serde_json::to_string(&descriptors).unwrap();
            assert!(!page.contains("exports"));
            assert!(!page.contains("relative_path"));
            assert!(!page.contains("data_context"));
        }
    }
    #[test]
    fn permissions_require_every_declared_producer_scope_before_execution() {
        let fixture = Fixture::new();
        let ids = vec!["reports".into()];
        assert!(validate_access(&fixture.context.scopes, &ids, &ids, None).is_err());
        assert!(validate_access(
            &fixture.context.scopes,
            &ids,
            &ids,
            Some(&Permission {
                version: 1,
                scopes: vec![]
            })
        )
        .is_err());
        assert_eq!(
            validate_access(
                &fixture.context.scopes,
                &ids,
                &ids,
                Some(&Permission {
                    version: 1,
                    scopes: ids.clone()
                })
            )
            .unwrap(),
            ids
        );
        assert!(validate_access(
            &fixture.context.scopes,
            &[],
            &ids,
            Some(&Permission {
                version: 1,
                scopes: ids.clone()
            })
        )
        .is_err());
        assert!(validate_access(&fixture.context.scopes, &ids, &[], None)
            .unwrap()
            .is_empty());
    }
    #[test]
    fn references_bind_context_binding_scope_content_and_exact_grant() {
        let fixture = Fixture::new();
        let nomination = fixture.put("value.txt", b"old", "text/plain");
        let (_, grants) = issue(&fixture.context, &[nomination.clone()]).unwrap();
        let grant = &grants[0];
        let mut context = fixture.context.clone();
        context.binding["catalog_sha256"] = json!("e".repeat(64));
        assert_eq!(
            read(&context, grant, &grant.reference).unwrap_err().code,
            "artifact.stale_binding"
        );
        context = fixture.context.clone();
        context.data_context_sha256 = "f".repeat(64);
        assert_eq!(
            read(&context, grant, &grant.reference).unwrap_err().code,
            "artifact.stale_context"
        );
        context = fixture.context.clone();
        context.allowed_scopes.clear();
        assert_eq!(
            read(&context, grant, &grant.reference).unwrap_err().code,
            "artifact.access_denied"
        );
        let mut tampered = grant.clone();
        tampered.relative_path = "other.txt".into();
        assert_eq!(
            read(&fixture.context, &tampered, &tampered.reference)
                .unwrap_err()
                .code,
            "artifact.invalid_request"
        );
        std::fs::write(fixture.context.data_root.join("exports/value.txt"), b"new").unwrap();
        assert_eq!(
            read(&fixture.context, grant, &grant.reference)
                .unwrap_err()
                .code,
            "artifact.content_changed"
        );
        let (_, fresh) = issue(&fixture.context, &[nomination]).unwrap();
        assert_ne!(fresh[0].reference, grant.reference);
        assert_eq!(fresh[0].binding, grant.binding);
        std::fs::remove_file(fixture.context.data_root.join("exports/value.txt")).unwrap();
        assert_eq!(
            read(&fixture.context, &fresh[0], &fresh[0].reference)
                .unwrap_err()
                .code,
            "artifact.not_found"
        );
    }
    #[test]
    fn issuance_is_atomic_and_does_not_revoke_previous_sets() {
        let fixture = Fixture::new();
        let good = fixture.put("ok.txt", b"ok", "text/plain");
        let (_, prior) = issue(&fixture.context, &[good.clone()]).unwrap();
        let mut missing = good.clone();
        missing.id = "missing".into();
        missing.path = "missing.txt".into();
        assert!(issue(&fixture.context, &[good, missing]).is_err());
        assert!(read(&fixture.context, &prior[0], &prior[0].reference).is_ok());
    }
    #[test]
    fn unsafe_locators_and_unsupported_formats_are_rejected() {
        let fixture = Fixture::new();
        let original = fixture.put("ok.txt", b"ok", "text/plain");
        for path in [
            "../secret",
            "/etc/passwd",
            "C:/secret",
            "//host/share",
            "a\\b",
            "CON",
            "a/../b",
            "a//b",
            "a./file",
            "a:stream",
            ".",
            "",
        ] {
            let mut bad = original.clone();
            bad.path = path.into();
            assert!(issue(&fixture.context, &[bad]).is_err(), "{path}");
        }
        let mut wrong = original;
        wrong.mime_type = "text/html".into();
        assert_eq!(
            issue(&fixture.context, &[wrong]).unwrap_err().code,
            "artifact.unsupported_type"
        );
        assert!(validate_format("text/plain", &[255]).is_err());
        for value in [
            br#"{"a":1,"a":2}"#.as_slice(),
            b"[1] true",
            b"\xff",
            b"null null",
        ] {
            assert!(validate_format("application/json", value).is_err());
        }
        assert!(validate_format(
            "application/json",
            format!("{}0{}", "[".repeat(33), "]".repeat(33)).as_bytes()
        )
        .is_err());
        assert!(validate_format(
            "application/json",
            format!("[{}]", vec!["0"; 65536].join(",")).as_bytes()
        )
        .is_err());
        assert!(validate_format("image/png", b"not png").is_err());
        let mut corrupt = png();
        corrupt[29] ^= 1;
        assert!(validate_format("image/png", &corrupt).is_err());
        let mut bad = wav();
        bad.push(0);
        assert!(validate_format("audio/wav", &bad).is_err());
    }
    #[test]
    fn exact_size_limit_and_hardlink_rejection() {
        let fixture = Fixture::new();
        let nomination = fixture.put("large.txt", &vec![b'x'; MAX_ARTIFACT_BYTES], "text/plain");
        let (_, grant) = issue(&fixture.context, &[nomination.clone()]).unwrap();
        assert!(read(&fixture.context, &grant[0], &grant[0].reference).is_ok());
        std::fs::write(
            fixture.context.data_root.join("exports/large.txt"),
            vec![b'x'; MAX_ARTIFACT_BYTES + 1],
        )
        .unwrap();
        assert_eq!(
            issue(&fixture.context, &[nomination]).unwrap_err().code,
            "artifact.size_limit"
        );
        let nomination = fixture.put("linked.txt", b"private", "text/plain");
        std::fs::hard_link(
            fixture.context.data_root.join("exports/linked.txt"),
            fixture.context.data_root.join("outside.txt"),
        )
        .unwrap();
        assert_eq!(
            issue(&fixture.context, &[nomination]).unwrap_err().code,
            "artifact.unsafe_file"
        );
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_at_leaf_scope_and_data_root_are_denied() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let nomination = fixture.put("file.txt", b"private", "text/plain");
        symlink(
            "file.txt",
            fixture.context.data_root.join("exports/link.txt"),
        )
        .unwrap();
        let mut linked = nomination.clone();
        linked.path = "link.txt".into();
        assert!(issue(&fixture.context, &[linked]).is_err());
        symlink("exports", fixture.context.data_root.join("linked-scope")).unwrap();
        let mut context = fixture.context.clone();
        context.scopes[0].path = "linked-scope".into();
        assert!(issue(&context, &[nomination.clone()]).is_err());
        let root_link = fixture.context.data_root.with_extension("link");
        symlink(&fixture.context.data_root, &root_link).unwrap();
        context = fixture.context.clone();
        context.data_root = root_link.clone();
        assert!(issue(&context, &[nomination]).is_err());
        std::fs::remove_file(root_link).unwrap();
    }
    #[test]
    fn format_chunk_counts_are_bounded() {
        let mut wave = b"RIFF\0\0\0\0WAVE".to_vec();
        for _ in 0..MAX_CHUNKS + 1 {
            wave.extend_from_slice(b"JUNK\0\0\0\0");
        }
        let size = (wave.len() - 8) as u32;
        wave[4..8].copy_from_slice(&size.to_le_bytes());
        assert!(validate_wav(&wave).is_err());
        let mut image = png();
        let tail = image.split_off(image.len() - 12);
        for _ in 0..MAX_CHUNKS {
            image.extend_from_slice(&0u32.to_be_bytes());
            image.extend_from_slice(b"tEXt");
            image.extend_from_slice(&crc32(b"tEXt").to_be_bytes());
        }
        image.extend(tail);
        assert!(validate_png(&image).is_err());
    }

    #[test]
    fn directories_and_missing_roots_are_not_initialized_or_read() {
        let fixture = Fixture::new();
        std::fs::create_dir(fixture.context.data_root.join("exports/directory")).unwrap();
        let nomination = Nomination {
            id: "directory".into(),
            scope: "reports".into(),
            path: "directory".into(),
            mime_type: "text/plain".into(),
        };
        assert_eq!(
            issue(&fixture.context, &[nomination.clone()])
                .unwrap_err()
                .code,
            "artifact.unsafe_file"
        );
        let mut context = fixture.context.clone();
        context.data_root = context.data_root.join("missing");
        assert_eq!(
            issue(&context, &[nomination]).unwrap_err().code,
            "artifact.not_found"
        );
        assert!(!context.data_root.exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_reparse_files_and_ancestors_are_denied() {
        use std::os::windows::fs::{symlink_dir, symlink_file};
        let fixture = Fixture::new();
        let mut nomination = fixture.put("value.txt", b"private", "text/plain");
        symlink_file(
            "value.txt",
            fixture.context.data_root.join("exports/link.txt"),
        )
        .unwrap();
        nomination.path = "link.txt".into();
        assert!(issue(&fixture.context, &[nomination.clone()]).is_err());
        symlink_dir("exports", fixture.context.data_root.join("linked-scope")).unwrap();
        let mut context = fixture.context.clone();
        context.scopes[0].path = "linked-scope".into();
        nomination.path = "value.txt".into();
        assert!(issue(&context, &[nomination]).is_err());
        std::fs::remove_dir(fixture.context.data_root.join("linked-scope")).unwrap();
    }
}
