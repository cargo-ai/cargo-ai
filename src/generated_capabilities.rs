//! Passive compatibility declarations embedded in generated executables.
//!
//! A declaration describes supported invocation settings, not provider acceptance,
//! authorization, or the safety of executing an artifact.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

#[path = "generated_capabilities/record.rs"]
mod record;
use record::{encoded_record, CLI_IDENTITY, CLOSE, IDENTITY, OPEN, RECORD_LEN, REVISION};

const READ_BLOCK_SIZE: usize = 64 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeCapabilities {
    runtime: &'static str,
    revision: u32,
    thinking: ThinkingCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_policy: Option<ExecutionPolicyCapabilities>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
struct ThinkingCapabilities {
    flags: &'static [&'static str],
    settings: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
struct ExecutionPolicyCapabilities {
    version: u32,
    boundaries: &'static [&'static str],
}

impl RuntimeCapabilities {
    pub fn supports_execution_policy(self) -> bool {
        self.execution_policy.is_some()
    }
    pub fn is_cli_run(self) -> bool {
        self.runtime == "cargo-ai.cli-run-runtime"
    }

    pub fn supports_thinking(self) -> bool {
        matches!(self.revision, 1 | 2 | REVISION)
    }

    pub fn supports_toggle(self) -> bool {
        matches!(self.revision, 2 | REVISION)
    }

    pub fn supports_exact_choice_flag(self) -> bool {
        matches!(self.revision, 2 | REVISION)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityReadError {
    Missing,
    Malformed,
    Unsupported,
    Conflicting,
    Oversized,
    NotRegularFile,
    Unreadable(io::ErrorKind),
}

pub fn decode_record(record: &[u8]) -> Result<RuntimeCapabilities, CapabilityReadError> {
    if record.len() != RECORD_LEN || !record.starts_with(&OPEN) || !record.ends_with(&CLOSE) {
        return Err(CapabilityReadError::Malformed);
    }
    let identity = &record[OPEN.len()..OPEN.len() + IDENTITY.len()];
    let cli_run = if identity == CLI_IDENTITY {
        true
    } else if identity == IDENTITY {
        false
    } else {
        return Err(CapabilityReadError::Malformed);
    };
    let payload = OPEN.len() + IDENTITY.len();
    let revision = u32::from_le_bytes(record[payload..payload + 4].try_into().unwrap());
    let mut legacy = encoded_record(cli_run);
    legacy[payload..payload + 4].copy_from_slice(&1u32.to_le_bytes());
    legacy[payload + 4..payload + 8].copy_from_slice(&0b1111u32.to_le_bytes());
    let mut prior = encoded_record(cli_run);
    prior[payload..payload + 4].copy_from_slice(&2u32.to_le_bytes());
    prior[payload + 4..payload + 8].copy_from_slice(&0b111_1111u32.to_le_bytes());
    if record != encoded_record(cli_run) && record != legacy && record != prior {
        return Err(CapabilityReadError::Unsupported);
    }
    Ok(RuntimeCapabilities {
        runtime: if cli_run {
            "cargo-ai.cli-run-runtime"
        } else {
            "cargo-ai.generated-runtime"
        },
        revision,
        execution_policy: (revision == REVISION).then_some(ExecutionPolicyCapabilities {
            version: 1,
            boundaries: &["root", "media", "descendants"],
        }),
        thinking: ThinkingCapabilities {
            flags: if revision == 1 {
                &["--thinking", "--thinking-provider-default"]
            } else {
                &[
                    "--thinking",
                    "--thinking-provider-default",
                    "--thinking-choice",
                ]
            },
            settings: if revision == 1 {
                &["choice", "provider_default"]
            } else {
                &["choice", "provider_default", "on", "off"]
            },
        },
    })
}

/// Reads only the selected regular artifact, with bounded memory and total reads.
/// Callers must apply their existing execution permission and path checks first.
pub fn capabilities_for_artifact(
    artifact: &Path,
) -> Result<RuntimeCapabilities, CapabilityReadError> {
    let metadata = std::fs::metadata(artifact)
        .map_err(|error| CapabilityReadError::Unreadable(error.kind()))?;
    if !metadata.is_file() {
        return Err(CapabilityReadError::NotRegularFile);
    }
    if metadata.len() > MAX_ARTIFACT_BYTES {
        return Err(CapabilityReadError::Oversized);
    }
    let file =
        File::open(artifact).map_err(|error| CapabilityReadError::Unreadable(error.kind()))?;
    capabilities_from_reader(file, MAX_ARTIFACT_BYTES)
}

fn capabilities_from_reader(
    mut reader: impl Read,
    limit: u64,
) -> Result<RuntimeCapabilities, CapabilityReadError> {
    let mut block = [0_u8; READ_BLOCK_SIZE];
    let mut window = Vec::with_capacity(READ_BLOCK_SIZE + RECORD_LEN - 1);
    let mut total = 0_u64;
    let mut found = None;
    loop {
        // One extra byte distinguishes an exact-limit artifact from an oversized
        // stream even if the artifact grows after its initial metadata read.
        let remaining = limit.saturating_sub(total).saturating_add(1);
        let block_len = remaining.min(block.len() as u64) as usize;
        let count = match reader.read(&mut block[..block_len]) {
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(CapabilityReadError::Unreadable(error.kind())),
        };
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > limit {
            return Err(CapabilityReadError::Oversized);
        }
        window.extend_from_slice(&block[..count]);
        for record in window.windows(RECORD_LEN) {
            // Standalone framing constants can occur in executables that also
            // read child declarations. Only a complete frame is a candidate.
            if !record.starts_with(&OPEN) || !record.ends_with(&CLOSE) {
                continue;
            }
            if found.is_some() {
                return Err(CapabilityReadError::Conflicting);
            }
            found = Some(decode_record(record)?);
        }
        let retain = window.len().min(RECORD_LEN - 1);
        window.drain(..window.len() - retain);
    }
    found.ok_or(CapabilityReadError::Missing)
}

#[cfg(test)]
pub(crate) fn test_record(revision: u32) -> Vec<u8> {
    let mut record = encoded_record(false);
    let payload = OPEN.len() + IDENTITY.len();
    record[payload..payload + 4].copy_from_slice(&revision.to_le_bytes());
    if revision == 2 {
        record[payload + 4..payload + 8].copy_from_slice(&0b111_1111u32.to_le_bytes());
    }
    record
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declares_only_exact_thinking_flags_and_settings() {
        let caps = decode_record(&encoded_record(false)).unwrap();
        assert!(caps.supports_thinking());
        assert!(!caps.is_cli_run());
        assert_eq!(
            serde_json::to_value(caps).unwrap(),
            serde_json::json!({
                "runtime": "cargo-ai.generated-runtime",
                "revision": 3,
                "execution_policy": {"version":1,"boundaries":["root","media","descendants"]},
                "thinking": {
                    "flags": ["--thinking", "--thinking-provider-default", "--thinking-choice"],
                    "settings": ["choice", "provider_default", "on", "off"]
                }
            })
        );
    }

    #[test]
    fn legacy_named_only_declaration_does_not_establish_boolean_support() {
        let mut record = encoded_record(false);
        let payload = OPEN.len() + IDENTITY.len();
        record[payload..payload + 4].copy_from_slice(&1u32.to_le_bytes());
        record[payload + 4..payload + 8].copy_from_slice(&0b1111u32.to_le_bytes());
        let legacy = decode_record(&record).unwrap();
        assert!(legacy.supports_thinking());
        assert!(!legacy.supports_toggle());
        assert!(!legacy.supports_exact_choice_flag());
        assert!(decode_record(&encoded_record(false))
            .unwrap()
            .supports_toggle());
    }

    #[test]
    fn thinking_support_does_not_imply_execution_policy_enforcement() {
        let older = decode_record(&test_record(2)).unwrap();
        assert!(older.supports_thinking());
        assert!(older.supports_toggle());
        assert!(!older.supports_execution_policy());
        let current = decode_record(&test_record(REVISION)).unwrap();
        assert!(current.supports_execution_policy());
    }

    #[test]
    fn cli_run_is_an_exact_distinct_runtime_declaration() {
        let cli = encoded_record(true);
        let generated = encoded_record(false);
        assert_eq!(cli.len(), generated.len());
        assert_ne!(cli, generated);
        let capabilities = capabilities_from_reader(cli.as_slice(), MAX_ARTIFACT_BYTES).unwrap();
        assert!(capabilities.is_cli_run());
        assert!(capabilities.supports_thinking());
        assert_eq!(
            serde_json::to_value(capabilities).unwrap()["runtime"],
            "cargo-ai.cli-run-runtime"
        );

        let mut malformed = cli.clone();
        malformed[OPEN.len() + CLI_IDENTITY.len() - 1] = b' ';
        assert_eq!(
            capabilities_from_reader(malformed.as_slice(), MAX_ARTIFACT_BYTES),
            Err(CapabilityReadError::Malformed)
        );
        let mut conflict = cli;
        conflict.extend_from_slice(&generated);
        assert_eq!(
            capabilities_from_reader(conflict.as_slice(), MAX_ARTIFACT_BYTES),
            Err(CapabilityReadError::Conflicting)
        );
    }

    #[test]
    fn streams_a_record_across_read_boundaries() {
        let record = encoded_record(false);
        for split in 1..RECORD_LEN {
            let mut artifact = vec![42; READ_BLOCK_SIZE - split];
            artifact.extend_from_slice(&record);
            artifact.extend_from_slice(&[13; RECORD_LEN]);
            assert!(
                capabilities_from_reader(artifact.as_slice(), MAX_ARTIFACT_BYTES)
                    .unwrap()
                    .supports_thinking()
            );
        }
    }

    #[test]
    fn opaque_and_source_examples_cannot_declare_support() {
        for source in [
            "#!/bin/sh\nprintf 'unexpected child execution' > execution-marker\n",
            "cargo-ai.generated-runtime --thinking --thinking-provider-default revision=1",
            include_str!("generated_capabilities.rs"),
        ] {
            assert_eq!(
                capabilities_from_reader(source.as_bytes(), MAX_ARTIFACT_BYTES),
                Err(CapabilityReadError::Missing)
            );
        }
        let numeric_source = format!(
            "static AGENT_RUNTIME_CAPABILITY_RECORD: [u8; {}] = {:?};",
            RECORD_LEN,
            encoded_record(false)
        );
        assert_eq!(
            capabilities_from_reader(numeric_source.as_bytes(), MAX_ARTIFACT_BYTES),
            Err(CapabilityReadError::Missing)
        );
    }

    #[test]
    fn malformed_and_unsupported_records_are_opaque() {
        let record = encoded_record(false);
        assert_eq!(
            decode_record(&record[..record.len() - 1]),
            Err(CapabilityReadError::Malformed)
        );
        let mut invalid_identity = record.clone();
        invalid_identity[OPEN.len()] ^= 1;
        assert_eq!(
            capabilities_from_reader(invalid_identity.as_slice(), MAX_ARTIFACT_BYTES),
            Err(CapabilityReadError::Malformed)
        );
        for index in [OPEN.len() + IDENTITY.len(), OPEN.len() + IDENTITY.len() + 4] {
            let mut unsupported = record.clone();
            unsupported[index] ^= 128;
            assert_eq!(
                capabilities_from_reader(unsupported.as_slice(), MAX_ARTIFACT_BYTES),
                Err(CapabilityReadError::Unsupported)
            );
        }
        for length in 0..record.len() {
            assert_eq!(
                capabilities_from_reader(&record[..length], MAX_ARTIFACT_BYTES),
                Err(CapabilityReadError::Missing)
            );
        }
    }

    #[test]
    fn multiple_declarations_cannot_establish_support() {
        let mut repeated = encoded_record(false);
        repeated.extend_from_slice(&encoded_record(false));
        assert_eq!(
            capabilities_from_reader(repeated.as_slice(), MAX_ARTIFACT_BYTES),
            Err(CapabilityReadError::Conflicting)
        );
        let mut different = encoded_record(false);
        let mut unsupported = encoded_record(false);
        unsupported[OPEN.len() + IDENTITY.len()] = 2;
        different.extend_from_slice(&unsupported);
        assert_eq!(
            capabilities_from_reader(different.as_slice(), MAX_ARTIFACT_BYTES),
            Err(CapabilityReadError::Conflicting)
        );
    }

    #[test]
    fn exact_read_limit_and_oversize_are_distinct() {
        let record = encoded_record(false);
        assert!(capabilities_from_reader(record.as_slice(), record.len() as u64).is_ok());
        assert_eq!(
            capabilities_from_reader(record.as_slice(), record.len() as u64 - 1),
            Err(CapabilityReadError::Oversized)
        );
        // A valid early declaration cannot hide bytes beyond the read limit.
        let mut oversized = record.clone();
        oversized.push(0);
        assert_eq!(
            capabilities_from_reader(oversized.as_slice(), record.len() as u64),
            Err(CapabilityReadError::Oversized)
        );
    }

    #[test]
    fn read_failure_after_a_declaration_cannot_establish_support() {
        struct FailingReader {
            record: io::Cursor<Vec<u8>>,
        }
        impl Read for FailingReader {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                if self.record.position() < self.record.get_ref().len() as u64 {
                    self.record.read(bytes)
                } else {
                    Err(io::Error::from(io::ErrorKind::PermissionDenied))
                }
            }
        }
        assert_eq!(
            capabilities_from_reader(
                FailingReader {
                    record: io::Cursor::new(encoded_record(false))
                },
                MAX_ARTIFACT_BYTES
            ),
            Err(CapabilityReadError::Unreadable(
                io::ErrorKind::PermissionDenied
            ))
        );
    }

    #[test]
    fn unreadable_and_non_regular_artifacts_are_opaque() {
        let missing = std::env::temp_dir().join(format!(
            "cargo-ai-capability-missing-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        assert_eq!(
            capabilities_for_artifact(&missing),
            Err(CapabilityReadError::Unreadable(io::ErrorKind::NotFound))
        );
        assert_eq!(
            capabilities_for_artifact(&std::env::temp_dir()),
            Err(CapabilityReadError::NotRegularFile)
        );
    }

    #[test]
    fn reads_the_selected_artifact_without_executing_it() {
        let directory = std::env::temp_dir().join(format!(
            "cargo-ai-capability-artifact-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&directory).unwrap();
        let artifact = directory.join("child");
        let marker = directory.join("execution-marker");
        let mut script =
            format!("#!/bin/sh\nprintf executed > '{}'\n", marker.display()).into_bytes();
        script.extend_from_slice(&encoded_record(false));
        std::fs::write(&artifact, &script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(capabilities_for_artifact(&artifact)
            .unwrap()
            .supports_thinking());
        assert!(!marker.exists(), "passive inspection executed the artifact");

        std::fs::write(&artifact, b"opaque child --thinking revision=1").unwrap();
        assert_eq!(
            capabilities_for_artifact(&artifact),
            Err(CapabilityReadError::Missing)
        );
        assert!(!marker.exists());
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
