//! Speech generation and transcription within the existing action runtime.
use super::*;
use std::io::Read;

const MAX_AUDIO_SOURCE_BYTES: u64 = 10 * 1024 * 1024;

fn scalar(
    value: Option<&crate::RunArg>,
    data: &serde_json::Value,
    field: &str,
) -> Result<String, String> {
    let value = match value.ok_or_else(|| format!("Missing required `{field}`."))? {
        crate::RunArg::Literal(value) => value.clone(),
        crate::RunArg::Variable(name) => lookup_action_variable(data, name)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("`{field}` variable '{name}' must resolve to a string."))?
            .to_string(),
    };
    if value.trim().is_empty() {
        return Err(format!("`{field}` must resolve to a nonempty string."));
    }
    Ok(value)
}

fn audio_format(path: &Path) -> Result<&'static str, String> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("wav") => Ok("wav"),
        Some("mp3") => Ok("mp3"),
        _ => Err("Audio paths must use a supported `.wav` or `.mp3` extension.".to_string()),
    }
}

fn read_source(root: &Path, relative: &str) -> Result<(Vec<u8>, &'static str), String> {
    let relative = audio_relative_path(Path::new(relative))?;
    let format = audio_format(&relative)?;
    let root = root
        .canonicalize()
        .map_err(|error| format!("Cannot resolve audio source root: {error}"))?;
    let path = root
        .join(&relative)
        .canonicalize()
        .map_err(|error| format!("Cannot resolve audio source: {error}"))?;
    if !path.starts_with(&root) {
        return Err("Audio source escapes its permitted root.".to_string());
    }
    let before_open = std::fs::metadata(&path)
        .map_err(|error| format!("Cannot inspect audio source: {error}"))?;
    if !before_open.is_file() || before_open.len() > MAX_AUDIO_SOURCE_BYTES {
        return Err("Audio source must be a regular file no larger than 10 MiB.".to_string());
    }
    let mut file =
        std::fs::File::open(&path).map_err(|error| format!("Cannot open audio source: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("Cannot inspect audio source: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_AUDIO_SOURCE_BYTES {
        return Err("Audio source must be a regular file no larger than 10 MiB.".to_string());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_AUDIO_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Cannot read audio source: {error}"))?;
    if bytes.len() as u64 > MAX_AUDIO_SOURCE_BYTES {
        return Err("Audio source exceeds 10 MiB.".to_string());
    }
    validate_audio_container(&bytes, format)?;
    Ok((
        bytes,
        if format == "wav" {
            "audio/wav"
        } else {
            "audio/mpeg"
        },
    ))
}

fn validate_audio_container(bytes: &[u8], format: &str) -> Result<(), String> {
    let valid = match format {
        "wav" => valid_wave(bytes),
        "mp3" => crate::providers::valid_mp3(bytes),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "Audio bytes do not match the requested {format} container."
        ))
    }
}

fn valid_wave(bytes: &[u8]) -> bool {
    if bytes.len() < 44 || !bytes.starts_with(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return false;
    }
    let declared = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    if declared.checked_add(8) != Some(bytes.len()) {
        return false;
    }
    let mut offset = 12usize;
    let mut format = false;
    let mut samples = false;
    while offset + 8 <= bytes.len() {
        let length = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let Some(end) = offset
            .checked_add(8)
            .and_then(|start| start.checked_add(length))
        else {
            return false;
        };
        if end > bytes.len() {
            return false;
        }
        if &bytes[offset..offset + 4] == b"fmt " {
            format = length >= 16
                && bytes[offset + 10..offset + 12] != [0, 0]
                && bytes[offset + 12..offset + 16] != [0, 0, 0, 0];
        }
        if &bytes[offset..offset + 4] == b"data" {
            samples |= length > 0;
        }
        offset = end + (length % 2);
    }
    format && samples && offset == bytes.len()
}

fn write_audio(path: &Path, bytes: &[u8], format: &str) -> Result<(), String> {
    validate_audio_container(bytes, format)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Cannot create audio output directory: {error}"))?;
    let staged = parent.join(format!(".cargo-ai-audio-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
            .map_err(|error| format!("Cannot stage audio output: {error}"))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("Cannot write audio output: {error}"))?;
        drop(file);
        std::fs::rename(&staged, path)
            .map_err(|error| format!("Cannot replace audio output: {error}"))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    result
}

pub(super) async fn run_audio_step(
    step: &crate::RunStep,
    data: &serde_json::Value,
    action_index: usize,
    action_name: &str,
    step_index: usize,
    provider_context: &ActionProviderContext,
    runtime_budget: InvocationRuntimeBudget,
) -> Result<Option<(String, String)>, String> {
    let speech = step.kind.eq_ignore_ascii_case("generate_audio");
    let kind = if speech {
        "generate_audio"
    } else {
        "transcribe_audio"
    };
    let selected = resolve_media_step_profile_context(
        step.profile.as_ref(),
        data,
        action_name,
        provider_context.inference_timeout_in_sec,
        kind,
    )
    .await?;
    let context = selected.as_ref().unwrap_or(provider_context);
    let capability = context.provider.capabilities();
    if !(if speech {
        capability.supports_generate_audio
    } else {
        capability.supports_transcribe_audio
    }) {
        return Err(format!(
            "{kind} is unsupported by {}.",
            context.provider.display_name()
        ));
    }
    if context.auth_mode != "api_key" || context.url.contains("chatgpt.com/backend-api/") {
        return Err(format!("{kind} requires a compatible API-key profile."));
    }
    let fixed_service = speech && context.provider == crate::providers::ProviderKind::Xai;
    let model = if fixed_service {
        if step.model.is_some() {
            return Err("xAI generate_audio does not accept an explicit model.".to_string());
        }
        None
    } else {
        Some(
            resolve_generate_image_model(
                step.model.as_ref(),
                data,
                action_name,
                selected.as_ref(),
                provider_context,
            )
            .map_err(|error| error.replace("generate_image", kind))?,
        )
    };
    // Resolve every step-owned input and path before starting a provider attempt.
    let text = if speech {
        let parts = step
            .text
            .as_deref()
            .ok_or_else(|| "generate_audio requires `text`.".to_string())?;
        let value = resolve_string_parts(parts, data, action_name, "text")?;
        if value.trim().is_empty() {
            return Err("generate_audio text must not be empty.".to_string());
        }
        value
    } else {
        String::new()
    };
    let voice = if speech {
        scalar(step.voice.as_ref(), data, "voice")?
    } else {
        String::new()
    };
    let mut output = None;
    let mut format = "";
    let mut source_bytes = Vec::new();
    let mut source_type = "";
    let mut source_name = String::new();
    if speech {
        let parts = step
            .path
            .as_deref()
            .ok_or_else(|| "generate_audio requires `path`.".to_string())?;
        let path = resolve_string_parts(parts, data, action_name, "path")?;
        format = audio_format(Path::new(&path))?;
        if context.provider == crate::providers::ProviderKind::Gemini && format != "wav" {
            return Err("Gemini generate_audio requires a `.wav` output path.".to_string());
        }
        output = Some(audio_output_path(provider_context, Path::new(&path))?);
    } else {
        source_name = scalar(step.audio_path.as_ref(), data, "audio.path")?;
        let root = audio_source_root(
            provider_context,
            matches!(step.audio_path, Some(crate::RunArg::Variable(_))),
        )?;
        (source_bytes, source_type) = read_source(&root, &source_name)?;
    }
    let captured_name = if speech {
        None
    } else {
        Some(
            step.output_variable
                .clone()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| "transcribe_audio requires `output_variable`.".to_string())?,
        )
    };
    let remaining = remaining_runtime_duration(runtime_budget, "before starting audio request")
        .map_err(|context| action_runtime_timeout_message(action_name, runtime_budget, &context))?;
    let usage_step = crate::usage_log::UsageStep {
        kind,
        action: Some(action_name.to_string()),
        step_index: Some(step_index),
    };
    let attempt = provider_context.usage_log.as_ref().map(|log| {
        log.start_provider_request_with_model(
            context.provider,
            usage_provider_profile(context),
            &context.auth_mode,
            model.as_deref(),
            usage_step.clone(),
        )
    });
    let remaining = remaining_runtime_duration(runtime_budget, "before dispatching audio request")
        .map_err(|message| {
            if let (Some(log), Some(attempt)) =
                (provider_context.usage_log.as_ref(), attempt.as_ref())
            {
                log.abort_provider_before_dispatch(attempt);
            }
            action_runtime_timeout_message(action_name, runtime_budget, &message)
        })?
        .min(remaining);
    let started = Instant::now();
    let response = tokio::time::timeout(remaining, async {
        if speech {
            crate::providers::send_speech_request(
                context.provider,
                &context.url,
                crate::providers::ProviderSpeechRequest {
                    model: model.as_deref(),
                    text: &text,
                    voice: &voice,
                    format,
                    timeout_in_sec: context.inference_timeout_in_sec,
                    token: &context.token,
                },
            )
            .await
            .map(|r| {
                (
                    Some(r.bytes),
                    None,
                    r.resolved_model,
                    r.provider_request_id,
                    r.usage,
                )
            })
        } else {
            crate::providers::send_transcription_request(
                context.provider,
                &context.url,
                crate::providers::ProviderTranscriptionRequest {
                    model: model.as_deref().unwrap_or_default(),
                    filename: Path::new(&source_name)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("audio.wav"),
                    audio_bytes: &source_bytes,
                    media_type: source_type,
                    timeout_in_sec: context.inference_timeout_in_sec,
                    token: &context.token,
                },
            )
            .await
            .map(|r| {
                (
                    None,
                    Some(r.text),
                    r.resolved_model,
                    r.provider_request_id,
                    r.usage,
                )
            })
        }
    })
    .await;
    let (resolved_model, request_id, usage, status, error) = match &response {
        Ok(Ok((_, _, model, id, usage))) => (
            model.as_deref(),
            id.as_deref(),
            usage.as_ref(),
            crate::usage_log::UsageStatus::Success,
            None,
        ),
        Ok(Err(error)) => (
            error.resolved_model.as_deref(),
            error.provider_request_id.as_deref(),
            error.usage.as_ref(),
            crate::usage_log::UsageStatus::Failed,
            Some(usage_provider_error(error)),
        ),
        Err(_) => (
            None,
            None,
            None,
            crate::usage_log::UsageStatus::Failed,
            Some(usage_timeout_error()),
        ),
    };
    if let (Some(log), Some(attempt)) = (provider_context.usage_log.as_ref(), attempt.as_ref()) {
        log.record_provider_request_with_model(
            crate::usage_log::UsageProviderRequest {
                attempt,
                provider: context.provider,
                profile_name: usage_provider_profile(context),
                auth_mode: &context.auth_mode,
                model: model.as_deref().unwrap_or_default(),
                resolved_model,
                provider_request_id: request_id,
                finish_reason: None,
                step: usage_step,
                usage,
                duration: started.elapsed(),
                status,
                error,
            },
            model.as_deref(),
        );
    }
    let (bytes, transcript, _, _, _) = response
        .map_err(|_| {
            action_runtime_timeout_message(
                action_name,
                runtime_budget,
                "while waiting for audio provider",
            )
        })?
        .map_err(|error| crate::providers::provider_error_messages(&error).join("\n"))?;
    if let Some(path) = output {
        write_audio(
            &path,
            bytes
                .as_deref()
                .ok_or_else(|| "No audio returned.".to_string())?,
            format,
        )?;
        print_action_line(
            action_index,
            action_name,
            &format!("wrote generated audio to '{}'.", path.display()),
        );
        Ok(None)
    } else {
        let transcript = transcript.ok_or_else(|| "No transcript returned.".to_string())?;
        if transcript.trim().is_empty() {
            return Err("No transcript returned for the audio source.".to_string());
        }
        Ok(Some((
            captured_name.expect("transcription capture was validated"),
            transcript,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> PathBuf {
        let path = std::env::temp_dir().join(format!("cargo-ai-audio-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn wave() -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&38u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&8000u32.to_le_bytes());
        bytes.extend_from_slice(&8000u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&8u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&[128, 128]);
        bytes
    }

    fn mp3() -> Vec<u8> {
        use base64::Engine as _;
        // A short, complete MPEG-2 Layer III silence fixture.
        base64::engine::general_purpose::STANDARD.decode("//NIxAAAAANIAAAAAExBTUVVVVVMQU1FMy4xMDBVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVV//NIxHwAAANIAAAAAFVVVVVVVVVMQU1FMy4xMDBVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVV//NIxHwAAANIAAAAAFVVVVVVVVVMQU1FMy4xMDBVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVV//NIxHwAAANIAAAAAFVVVVVVVVVMQU1FMy4xMDBVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVV//NIxHwAAANIAAAAAFVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVV").unwrap()
    }

    #[test]
    fn audio_container_rejects_mislabeled_truncated_and_tag_only_files() {
        assert!(validate_audio_container(&wave(), "wav").is_ok());
        assert!(validate_audio_container(&wave()[..44], "wav").is_err());
        assert!(validate_audio_container(&wave(), "mp3").is_err());
        assert!(validate_audio_container(b"ID3\x04\0\0\0\0\0\0", "mp3").is_err());
        assert!(validate_audio_container(&[0xff, 0xfb, 0x90, 0x64, 0], "mp3").is_err());
        let complete = mp3();
        assert!(validate_audio_container(&complete, "mp3").is_ok());
        assert!(validate_audio_container(&complete[..complete.len() - 1], "mp3").is_err());
        let mut tagged = b"ID3\x04\0\0\0\0\0\0".to_vec();
        tagged.extend_from_slice(&complete);
        tagged.extend_from_slice(b"TAG");
        tagged.extend_from_slice(&[0; 125]);
        assert!(validate_audio_container(&tagged, "mp3").is_ok());
        tagged.pop();
        assert!(validate_audio_container(&tagged, "mp3").is_err());
    }

    #[test]
    fn audio_writes_replace_only_valid_complete_output_and_remove_failed_staging() {
        let root = fixture_root();
        let output = root.join("speech.wav");
        std::fs::write(&output, b"previous").unwrap();
        assert!(write_audio(&output, b"invalid", "wav").is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"previous");
        write_audio(&output, &wave(), "wav").unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), wave());
        let mp3_output = root.join("speech.mp3");
        std::fs::write(&mp3_output, mp3()).unwrap();
        assert!(write_audio(&mp3_output, &[0xff, 0xfb, 0x90, 0x64, 0], "mp3").is_err());
        assert_eq!(std::fs::read(&mp3_output).unwrap(), mp3());
        let truncated = mp3();
        assert!(write_audio(&mp3_output, &truncated[..truncated.len() - 1], "mp3").is_err());
        assert_eq!(std::fs::read(&mp3_output).unwrap(), mp3());
        let directory = root.join("directory.wav");
        std::fs::create_dir(&directory).unwrap();
        assert!(write_audio(&directory, &wave(), "wav").is_err());
        assert!(directory.is_dir());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 3);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn audio_source_enforces_container_size_and_relative_root() {
        let root = fixture_root();
        std::fs::write(root.join("source.wav"), wave()).unwrap();
        assert_eq!(read_source(&root, "source.wav").unwrap().1, "audio/wav");
        std::fs::write(root.join("source.mp3"), mp3()).unwrap();
        assert_eq!(read_source(&root, "source.mp3").unwrap().1, "audio/mpeg");
        std::fs::write(root.join("bad.mp3"), [0xff, 0xfb, 0x90, 0x64, 0]).unwrap();
        assert!(read_source(&root, "bad.mp3").is_err());
        assert!(read_source(&root, "../source.wav").is_err());
        assert!(read_source(&root, root.join("source.wav").to_str().unwrap()).is_err());
        let file = std::fs::File::create(root.join("large.wav")).unwrap();
        file.set_len(MAX_AUDIO_SOURCE_BYTES + 1).unwrap();
        assert!(read_source(&root, "large.wav")
            .unwrap_err()
            .contains("10 MiB"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn audio_source_rejects_symlink_escape() {
        let root = fixture_root();
        let outside = fixture_root();
        std::fs::write(outside.join("source.wav"), wave()).unwrap();
        std::os::unix::fs::symlink(outside.join("source.wav"), root.join("link.wav")).unwrap();
        assert!(read_source(&root, "link.wav")
            .unwrap_err()
            .contains("escapes"));
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }
}
