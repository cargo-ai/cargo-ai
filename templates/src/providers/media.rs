use super::{
    error::sanitized_http_error_body,
    runtime::{ProviderFacts, ProviderUsage},
    ProviderError, ProviderKind,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use reqwest::{header, redirect::Policy, Client, Url};
use serde_json::{json, Value};
use std::time::Duration;

const MAX_RESPONSE_BYTES: usize = 30 * 1024 * 1024;
const MAX_AUDIO_BYTES: usize = 20 * 1024 * 1024;
const MAX_TRANSCRIPTION_BYTES: usize = 10 * 1024 * 1024;

pub(crate) struct ProviderSpeechRequest<'a> {
    pub(crate) model: Option<&'a str>,
    pub(crate) text: &'a str,
    pub(crate) voice: &'a str,
    pub(crate) format: &'a str,
    pub(crate) timeout_in_sec: u64,
    pub(crate) token: &'a str,
}

pub(crate) struct ProviderTranscriptionRequest<'a> {
    pub(crate) model: &'a str,
    pub(crate) filename: &'a str,
    pub(crate) audio_bytes: &'a [u8],
    pub(crate) media_type: &'a str,
    pub(crate) timeout_in_sec: u64,
    pub(crate) token: &'a str,
}

#[derive(Debug)]
pub(crate) struct ProviderAudioResponse {
    pub(crate) bytes: Vec<u8>,
    pub(crate) provider_request_id: Option<String>,
    pub(crate) resolved_model: Option<String>,
    pub(crate) usage: Option<ProviderUsage>,
}

#[derive(Debug)]
pub(crate) struct ProviderTranscriptResponse {
    pub(crate) text: String,
    pub(crate) provider_request_id: Option<String>,
    pub(crate) resolved_model: Option<String>,
    pub(crate) usage: Option<ProviderUsage>,
}

pub(super) fn endpoint(
    provider: ProviderKind,
    url: &str,
    suffix: &str,
) -> Result<Url, ProviderError> {
    let mut parsed = Url::parse(url)
        .map_err(|_| ProviderError::invalid_request(provider, "Invalid provider URL."))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.username() != ""
        || parsed.password().is_some()
    {
        return Err(ProviderError::invalid_request(
            provider,
            "Provider URL must be HTTP(S) without user information.",
        ));
    }
    let path = parsed.path();
    let prefix = path
        .find("/v1beta/")
        .or_else(|| path.find("/v1/"))
        .map(|position| &path[..position])
        .unwrap_or("");
    parsed.set_path(&format!("{prefix}{suffix}"));
    parsed.set_query(None);
    parsed.set_fragment(None);
    Ok(parsed)
}

pub(super) fn client(provider: ProviderKind, timeout_in_sec: u64) -> Result<Client, ProviderError> {
    Client::builder()
        .timeout(Duration::from_secs(timeout_in_sec))
        .redirect(Policy::none())
        .build()
        .map_err(|error| ProviderError::from_reqwest(provider, error))
}

pub(super) async fn limited_body(
    provider: ProviderKind,
    mut response: reqwest::Response,
    cap: usize,
) -> Result<Vec<u8>, ProviderError> {
    if response
        .content_length()
        .is_some_and(|length| length > cap as u64)
    {
        return Err(ProviderError::invalid_response(
            provider,
            "Provider response exceeded the size limit.",
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| ProviderError::from_reqwest(provider, error))?
    {
        if chunk.len() > cap.saturating_sub(bytes.len()) {
            return Err(ProviderError::invalid_response(
                provider,
                "Provider response exceeded the size limit.",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub(super) async fn checked_body(
    provider: ProviderKind,
    response: reqwest::Response,
    token: &str,
    cap: usize,
) -> Result<(Vec<u8>, ProviderFacts), ProviderError> {
    let status = response.status();
    let request_id = response
        .headers()
        .get("x-request-id")
        .or_else(|| response.headers().get("request-id"))
        .and_then(|header| header.to_str().ok())
        .map(str::to_owned);
    let body = limited_body(provider, response, cap).await?;
    if !status.is_success() {
        return Err(ProviderError::from_http_status(
            provider,
            status,
            &sanitized_http_error_body(provider, &body),
        )
        .with_request_id(request_id.as_deref(), token));
    }
    let facts = ProviderFacts::from_body(&body, provider)
        .with_request_id(request_id.as_deref())
        .redact_token(token);
    Ok((body, facts))
}

pub(super) fn json_body(provider: ProviderKind, body: &[u8]) -> Result<Value, ProviderError> {
    serde_json::from_slice(body)
        .map_err(|_| ProviderError::invalid_response(provider, "Provider returned malformed JSON."))
}

fn decoded(provider: ProviderKind, encoded: &str) -> Result<Vec<u8>, ProviderError> {
    if encoded.len() > ((MAX_AUDIO_BYTES + 2) / 3) * 4 + 16 {
        return Err(ProviderError::invalid_response(
            provider,
            "Encoded audio exceeded the size limit.",
        ));
    }
    BASE64_STANDARD.decode(encoded).map_err(|_| {
        ProviderError::invalid_response(provider, "Provider returned invalid base64 audio.")
    })
}

pub(crate) fn valid_mp3(bytes: &[u8]) -> bool {
    let mut offset = 0usize;
    if bytes.starts_with(b"ID3") {
        if bytes.len() < 10
            || !(2..=4).contains(&bytes[3])
            || bytes[4] == 0xff
            || bytes[6..10].iter().any(|byte| byte & 0x80 != 0)
        {
            return false;
        }
        let permitted_flags = match bytes[3] {
            2 => 0xc0,
            3 => 0xe0,
            _ => 0xf0,
        };
        if bytes[5] & !permitted_flags != 0 {
            return false;
        }
        let tag_size = bytes[6..10]
            .iter()
            .fold(0usize, |size, byte| (size << 7) | *byte as usize);
        offset = 10 + tag_size;
        if offset > bytes.len() {
            return false;
        }
        if bytes[3] == 4 && bytes[5] & 0x10 != 0 {
            let Some(footer) = bytes.get(offset..offset + 10) else {
                return false;
            };
            if &footer[..3] != b"3DI" || footer[3..] != bytes[3..10] {
                return false;
            }
            offset += 10;
        }
    }
    let mut frames = 0usize;
    while offset < bytes.len() {
        // ID3v1 is the only permitted trailing tag; it cannot substitute for audio.
        if bytes.len() - offset == 128 && bytes[offset..].starts_with(b"TAG") {
            return frames > 0;
        }
        let Some(header) = bytes.get(offset..offset + 4) else {
            return false;
        };
        let version = (header[1] >> 3) & 3;
        let bitrate_index = (header[2] >> 4) as usize;
        let rate_index = ((header[2] >> 2) & 3) as usize;
        if header[0] != 0xff
            || header[1] & 0xe0 != 0xe0
            || header[1] & 0x06 != 0x02
            || version == 1
            || bitrate_index == 0
            || bitrate_index == 15
            || rate_index == 3
            || header[3] & 3 == 2
        {
            return false;
        }
        let bitrate = if version == 3 {
            [
                0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
            ][bitrate_index]
        } else {
            [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160][bitrate_index]
        };
        let sample_rate = [44100, 48000, 32000][rate_index]
            / match version {
                3 => 1,
                2 => 2,
                _ => 4,
            };
        // Layer III frame size includes its header, optional CRC and padding.
        let samples = if version == 3 { 1152 } else { 576 };
        let frame_size = samples * bitrate * 125 / sample_rate + usize::from(header[2] & 0x02 != 0);
        let Some(end) = offset.checked_add(frame_size) else {
            return false;
        };
        if end > bytes.len() {
            return false;
        }
        offset = end;
        frames += 1;
    }
    frames > 0
}

fn check_audio(provider: ProviderKind, bytes: &[u8], format: &str) -> Result<(), ProviderError> {
    if bytes.len() > MAX_AUDIO_BYTES {
        return Err(ProviderError::invalid_response(
            provider,
            "Generated audio exceeded the size limit.",
        ));
    }
    let valid = match format {
        "wav" => bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE",
        "mp3" => valid_mp3(bytes),
        _ => {
            return Err(ProviderError::invalid_request(
                provider,
                "Audio output must be WAV or MP3.",
            ))
        }
    };
    if valid {
        Ok(())
    } else {
        Err(ProviderError::invalid_response(
            provider,
            "Generated audio did not match the requested file format.",
        ))
    }
}

fn normalize_openai_streaming_wav(mut bytes: Vec<u8>) -> Result<Vec<u8>, ProviderError> {
    if bytes.len() < 12 || !bytes.starts_with(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Ok(bytes);
    }
    if bytes[4..8] != u32::MAX.to_le_bytes() {
        return Ok(bytes);
    }

    let invalid = || {
        ProviderError::invalid_response(
            ProviderKind::OpenAi,
            "OpenAI returned an invalid streaming WAV response.",
        )
    };
    let mut offset = 12usize;
    let mut block_align = None;
    while offset + 8 <= bytes.len() {
        let chunk_start = offset + 8;
        let length = u32::from_le_bytes(bytes[offset + 4..chunk_start].try_into().unwrap());
        if &bytes[offset..offset + 4] == b"data" {
            if length != u32::MAX || block_align.is_none() {
                return Err(invalid());
            }
            let sample_len = bytes.len() - chunk_start;
            if sample_len == 0 || sample_len % block_align.unwrap() != 0 || sample_len % 2 != 0 {
                return Err(invalid());
            }
            let finite_sample_len = u32::try_from(sample_len).map_err(|_| invalid())?;
            let finite_riff_len = u32::try_from(bytes.len() - 8).map_err(|_| invalid())?;
            bytes[4..8].copy_from_slice(&finite_riff_len.to_le_bytes());
            bytes[offset + 4..chunk_start].copy_from_slice(&finite_sample_len.to_le_bytes());
            return Ok(bytes);
        }
        let end = chunk_start
            .checked_add(length as usize)
            .and_then(|end| end.checked_add((length % 2) as usize))
            .filter(|end| *end <= bytes.len())
            .ok_or_else(invalid)?;
        if &bytes[offset..offset + 4] == b"fmt " {
            if block_align.is_some() || length < 16 {
                return Err(invalid());
            }
            let format =
                u16::from_le_bytes(bytes[chunk_start..chunk_start + 2].try_into().unwrap());
            let channels =
                u16::from_le_bytes(bytes[chunk_start + 2..chunk_start + 4].try_into().unwrap());
            let sample_rate =
                u32::from_le_bytes(bytes[chunk_start + 4..chunk_start + 8].try_into().unwrap());
            let byte_rate =
                u32::from_le_bytes(bytes[chunk_start + 8..chunk_start + 12].try_into().unwrap());
            let alignment = u16::from_le_bytes(
                bytes[chunk_start + 12..chunk_start + 14]
                    .try_into()
                    .unwrap(),
            );
            let bits = u16::from_le_bytes(
                bytes[chunk_start + 14..chunk_start + 16]
                    .try_into()
                    .unwrap(),
            );
            let expected_align = channels.checked_mul(bits / 8);
            if format != 1
                || channels == 0
                || sample_rate == 0
                || bits == 0
                || bits % 8 != 0
                || expected_align != Some(alignment)
                || alignment == 0
                || sample_rate.checked_mul(u32::from(alignment)) != Some(byte_rate)
            {
                return Err(invalid());
            }
            block_align = Some(usize::from(alignment));
        }
        offset = end;
    }
    Err(invalid())
}

fn speech_response(
    provider: ProviderKind,
    bytes: Vec<u8>,
    facts: ProviderFacts,
    format: &str,
) -> Result<ProviderAudioResponse, ProviderError> {
    check_audio(provider, &bytes, format).map_err(|error| facts.error(error))?;
    Ok(ProviderAudioResponse {
        bytes,
        provider_request_id: facts.provider_request_id,
        resolved_model: facts.resolved_model,
        usage: facts.usage,
    })
}

fn transcript_response(
    provider: ProviderKind,
    text: String,
    facts: ProviderFacts,
) -> Result<ProviderTranscriptResponse, ProviderError> {
    if text.trim().is_empty() {
        return Err(facts.error(ProviderError::invalid_response(
            provider,
            "Provider returned no transcript text.",
        )));
    }
    Ok(ProviderTranscriptResponse {
        text,
        provider_request_id: facts.provider_request_id,
        resolved_model: facts.resolved_model,
        usage: facts.usage,
    })
}

pub(crate) async fn send_speech_request(
    provider: ProviderKind,
    url: &str,
    request: ProviderSpeechRequest<'_>,
) -> Result<ProviderAudioResponse, ProviderError> {
    if !provider.capabilities().supports_generate_audio {
        return Err(ProviderError::invalid_request(
            provider,
            "Speech generation is not supported by this provider.",
        ));
    }
    if request.text.trim().is_empty() || request.voice.trim().is_empty() {
        return Err(ProviderError::invalid_request(
            provider,
            "Speech text and voice are required.",
        ));
    }
    if !matches!(request.format, "wav" | "mp3")
        || (provider == ProviderKind::Gemini && request.format != "wav")
    {
        return Err(ProviderError::invalid_request(
            provider,
            "This provider does not support the requested speech format.",
        ));
    }
    let client = client(provider, request.timeout_in_sec)?;
    let (endpoint_url, body) = match provider {
        ProviderKind::OpenAi => {
            if url.contains("chatgpt.com/") {
                return Err(ProviderError::invalid_request(
                    provider,
                    "Speech generation requires OpenAI API-key transport.",
                ));
            }
            (
                endpoint(provider, url, "/v1/audio/speech")?,
                json!({"model": required_model(provider, request.model)?, "input": request.text, "voice": request.voice, "response_format": request.format}),
            )
        }
        ProviderKind::Gemini => (
            endpoint(provider, url, "/v1beta/interactions")?,
            json!({"model": required_model(provider, request.model)?, "input": [{"type":"user_input","content":[{"type":"text","text":request.text}]}], "response_format":{"type":"audio"}, "generation_config":{"speech_config":[{"voice":request.voice}]}, "store":false}),
        ),
        ProviderKind::Mistral => (
            endpoint(provider, url, "/v1/audio/speech")?,
            json!({"model": required_model(provider, request.model)?, "input":request.text, "voice_id":request.voice, "response_format":request.format, "stream":false}),
        ),
        ProviderKind::Xai => {
            if request.model.is_some() {
                return Err(ProviderError::invalid_request(
                    provider,
                    "xAI speech is a fixed service and does not accept a model.",
                ));
            }
            if request.text.chars().count() > 60_000 {
                return Err(ProviderError::invalid_request(
                    provider,
                    "xAI speech input exceeds 60,000 characters.",
                ));
            }
            (
                endpoint(provider, url, "/v1/tts")?,
                json!({"text":request.text,"voice_id":request.voice,"language":"auto","output_format":{"codec":request.format}}),
            )
        }
        _ => unreachable!(),
    };
    let builder = client
        .post(endpoint_url)
        .header(header::CONTENT_TYPE, "application/json")
        .json(&body);
    let builder = if provider == ProviderKind::Gemini {
        builder.header("x-goog-api-key", request.token)
    } else {
        builder.bearer_auth(request.token)
    };
    let response = builder
        .send()
        .await
        .map_err(|error| ProviderError::from_reqwest(provider, error))?;
    let (body, facts) = checked_body(provider, response, request.token, MAX_RESPONSE_BYTES).await?;
    let bytes = match provider {
        ProviderKind::OpenAi if request.format == "wav" => {
            normalize_openai_streaming_wav(body).map_err(|error| facts.error(error))?
        }
        ProviderKind::OpenAi | ProviderKind::Xai => body,
        ProviderKind::Mistral => {
            let value = json_body(provider, &body).map_err(|error| facts.error(error))?;
            let encoded = value
                .get("audio_data")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    facts.error(ProviderError::invalid_response(
                        provider,
                        "Mistral returned no audio_data.",
                    ))
                })?;
            decoded(provider, encoded).map_err(|error| facts.error(error))?
        }
        ProviderKind::Gemini => {
            let value = json_body(provider, &body).map_err(|error| facts.error(error))?;
            let chunks = value["steps"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|step| step["type"] == "model_output")
                .flat_map(|step| step["content"].as_array().into_iter().flatten())
                .filter(|content| content["type"] == "audio")
                .filter_map(|content| content["data"].as_str())
                .collect::<Vec<_>>();
            if chunks.len() != 1 {
                return Err(facts.error(ProviderError::invalid_response(
                    provider,
                    "Gemini did not return exactly one audio result.",
                )));
            }
            decoded(provider, chunks[0]).map_err(|error| facts.error(error))?
        }
        _ => unreachable!(),
    };
    speech_response(provider, bytes, facts, request.format)
}

fn required_model<'a>(
    provider: ProviderKind,
    model: Option<&'a str>,
) -> Result<&'a str, ProviderError> {
    model
        .filter(|model| !model.trim().is_empty())
        .ok_or_else(|| {
            ProviderError::invalid_request(
                provider,
                "A speech model is required for this provider.",
            )
        })
}

fn multipart_audio(
    request: &ProviderTranscriptionRequest<'_>,
    provider: ProviderKind,
) -> Result<(String, Vec<u8>), ProviderError> {
    let boundary = "cargo-ai-audio-boundary";
    let filename = request
        .filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("audio");
    if filename.is_empty() || filename.contains(['"', '\r', '\n']) {
        return Err(ProviderError::invalid_request(
            provider,
            "Invalid audio filename.",
        ));
    }
    let mut body = Vec::new();
    let field = |body: &mut Vec<u8>, key: &str, value: &str| {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{key}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    };
    field(&mut body, "model", request.model);
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {}\r\n\r\n", request.media_type).as_bytes());
    body.extend_from_slice(request.audio_bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Ok((format!("multipart/form-data; boundary={boundary}"), body))
}

pub(crate) async fn send_transcription_request(
    provider: ProviderKind,
    url: &str,
    request: ProviderTranscriptionRequest<'_>,
) -> Result<ProviderTranscriptResponse, ProviderError> {
    if !provider.capabilities().supports_transcribe_audio {
        return Err(ProviderError::invalid_request(
            provider,
            "Audio transcription is not supported by this provider.",
        ));
    }
    if request.model.trim().is_empty()
        || request.audio_bytes.is_empty()
        || request.audio_bytes.len() > MAX_TRANSCRIPTION_BYTES
        || !matches!(request.media_type, "audio/wav" | "audio/mpeg")
    {
        return Err(ProviderError::invalid_request(
            provider,
            "Transcription requires a model and a WAV or MP3 file up to 10 MiB.",
        ));
    }
    let client = client(provider, request.timeout_in_sec)?;
    let response = match provider {
        ProviderKind::Gemini => {
            let body = json!({"model":request.model,"input":[{"type":"audio","data":BASE64_STANDARD.encode(request.audio_bytes),"mime_type":request.media_type}],"store":false});
            client
                .post(endpoint(provider, url, "/v1beta/interactions")?)
                .header("x-goog-api-key", request.token)
                .json(&body)
                .send()
                .await
                .map_err(|error| ProviderError::from_reqwest(provider, error))?
        }
        ProviderKind::OpenAi | ProviderKind::Mistral | ProviderKind::Xai => {
            if provider == ProviderKind::OpenAi && url.contains("chatgpt.com/") {
                return Err(ProviderError::invalid_request(
                    provider,
                    "Transcription requires OpenAI API-key transport.",
                ));
            }
            let suffix = if provider == ProviderKind::Xai {
                "/v1/stt"
            } else {
                "/v1/audio/transcriptions"
            };
            let (content_type, body) = multipart_audio(&request, provider)?;
            client
                .post(endpoint(provider, url, suffix)?)
                .bearer_auth(request.token)
                .header(header::CONTENT_TYPE, content_type)
                .body(body)
                .send()
                .await
                .map_err(|error| ProviderError::from_reqwest(provider, error))?
        }
        _ => unreachable!(),
    };
    let (body, facts) = checked_body(provider, response, request.token, MAX_RESPONSE_BYTES).await?;
    let value = json_body(provider, &body).map_err(|error| facts.error(error))?;
    let text = if provider == ProviderKind::Gemini {
        let chunks = value["steps"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|step| step["type"] == "model_output")
            .flat_map(|step| step["content"].as_array().into_iter().flatten())
            .filter(|content| content["type"] == "text")
            .filter_map(|content| content["text"].as_str())
            .collect::<Vec<_>>();
        if chunks.len() != 1 {
            return Err(facts.error(ProviderError::invalid_response(
                provider,
                "Gemini did not return exactly one transcript.",
            )));
        }
        chunks[0].to_owned()
    } else {
        value["text"]
            .as_str()
            .ok_or_else(|| {
                facts.error(ProviderError::invalid_response(
                    provider,
                    "Provider returned no transcript text.",
                ))
            })?
            .to_owned()
    };
    transcript_response(provider, text, facts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    const WAV: &[u8] = b"RIFF\x04\x00\x00\x00WAVE";

    fn pcm_wave(streaming_lengths: bool) -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&if streaming_lengths { u32::MAX } else { 40 }.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&24_000u32.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&if streaming_lengths { u32::MAX } else { 4 }.to_le_bytes());
        bytes.extend_from_slice(&[0x10, 0x20, 0x30, 0x40]);
        bytes
    }

    #[test]
    fn openai_streaming_wav_lengths_are_finite_and_samples_are_preserved() {
        let streamed = pcm_wave(true);
        let normalized = normalize_openai_streaming_wav(streamed.clone()).unwrap();
        assert_eq!(&normalized[4..8], &40u32.to_le_bytes());
        assert_eq!(&normalized[40..44], &4u32.to_le_bytes());
        assert_eq!(&normalized[8..40], &streamed[8..40]);
        assert_eq!(&normalized[44..], &streamed[44..]);
        let ordinary = pcm_wave(false);
        assert_eq!(
            normalize_openai_streaming_wav(ordinary.clone()).unwrap(),
            ordinary
        );
    }

    #[test]
    fn openai_streaming_wav_rejects_malformed_or_incomplete_pcm() {
        let mut empty = pcm_wave(true);
        empty.truncate(44);
        assert!(normalize_openai_streaming_wav(empty).is_err());

        let mut nonaligned = pcm_wave(true);
        nonaligned.pop();
        assert!(normalize_openai_streaming_wav(nonaligned).is_err());

        let mut incomplete_chunk = pcm_wave(true);
        incomplete_chunk[16..20].copy_from_slice(&32u32.to_le_bytes());
        assert!(normalize_openai_streaming_wav(incomplete_chunk).is_err());

        let mut finite_data_claim = pcm_wave(true);
        finite_data_claim[40..44].copy_from_slice(&4u32.to_le_bytes());
        assert!(normalize_openai_streaming_wav(finite_data_claim).is_err());

        let mut invalid_pcm = pcm_wave(true);
        invalid_pcm[34..36].copy_from_slice(&0u16.to_le_bytes());
        assert!(normalize_openai_streaming_wav(invalid_pcm).is_err());
    }

    #[tokio::test]
    async fn openai_streaming_wav_response_is_normalized_after_download() {
        let mut server = Server::new_async().await;
        let fixture = server
            .mock("POST", "/v1/audio/speech")
            .with_status(200)
            .with_body(pcm_wave(true))
            .create_async()
            .await;
        let result = send_speech_request(
            ProviderKind::OpenAi,
            &server.url(),
            ProviderSpeechRequest {
                model: Some("speech-model"),
                text: "hello",
                voice: "coral",
                format: "wav",
                timeout_in_sec: 5,
                token: "token",
            },
        )
        .await
        .unwrap();
        assert_eq!(result.bytes, pcm_wave(false));
        fixture.assert_async().await;
    }

    #[tokio::test]
    async fn speech_uses_native_requests_and_decodes_all_four_responses() {
        for provider in [
            ProviderKind::OpenAi,
            ProviderKind::Gemini,
            ProviderKind::Mistral,
            ProviderKind::Xai,
        ] {
            let mut server = Server::new_async().await;
            let (path,response)=match provider {
                ProviderKind::OpenAi => ("/v1/audio/speech",WAV.to_vec()),
                ProviderKind::Gemini => ("/v1beta/interactions",json!({"steps":[{"type":"model_output","content":[{"type":"audio","data":BASE64_STANDARD.encode(WAV)}]}]}).to_string().into_bytes()),
                ProviderKind::Mistral => ("/v1/audio/speech",json!({"audio_data":BASE64_STANDARD.encode(WAV)}).to_string().into_bytes()),
                ProviderKind::Xai => ("/v1/tts",WAV.to_vec()),
                _=>unreachable!(),
            };
            let fixture = server
                .mock("POST", path)
                .match_body(Matcher::Regex("private-script".into()))
                .with_status(200)
                .with_header("x-request-id", "speech-fixture")
                .with_body(response)
                .create_async()
                .await;
            let result = send_speech_request(
                provider,
                &server.url(),
                ProviderSpeechRequest {
                    model: (provider != ProviderKind::Xai).then_some("speech-model"),
                    text: "private-script",
                    voice: "voice-id",
                    format: "wav",
                    timeout_in_sec: 5,
                    token: "secret-token",
                },
            )
            .await
            .unwrap();
            assert_eq!(result.bytes, WAV);
            assert_eq!(
                result.provider_request_id.as_deref(),
                Some("speech-fixture")
            );
            fixture.assert_async().await;
        }
    }

    #[tokio::test]
    async fn transcription_uses_native_requests_and_preserves_text() {
        for provider in [
            ProviderKind::OpenAi,
            ProviderKind::Gemini,
            ProviderKind::Mistral,
            ProviderKind::Xai,
        ] {
            let mut server = Server::new_async().await;
            let path = match provider {
                ProviderKind::Gemini => "/v1beta/interactions",
                ProviderKind::Xai => "/v1/stt",
                _ => "/v1/audio/transcriptions",
            };
            let body = if provider == ProviderKind::Gemini {
                json!({"steps":[{"type":"model_output","content":[{"type":"text","text":" Spoken words. "}]}]})
            } else {
                json!({"text":" Spoken words. "})
            };
            let fixture = server
                .mock("POST", path)
                .match_body(Matcher::Regex("speech-model".into()))
                .with_status(200)
                .with_body(body.to_string())
                .create_async()
                .await;
            let result = send_transcription_request(
                provider,
                &server.url(),
                ProviderTranscriptionRequest {
                    model: "speech-model",
                    filename: "recording.wav",
                    audio_bytes: WAV,
                    media_type: "audio/wav",
                    timeout_in_sec: 5,
                    token: "secret-token",
                },
            )
            .await
            .unwrap();
            assert_eq!(result.text, " Spoken words. ");
            fixture.assert_async().await;
        }
    }

    #[tokio::test]
    async fn rejects_fixed_service_model_invalid_audio_and_empty_transcript() {
        let error = send_speech_request(
            ProviderKind::Xai,
            "http://unused.invalid",
            ProviderSpeechRequest {
                model: Some("ignored-model"),
                text: "hello",
                voice: "eve",
                format: "mp3",
                timeout_in_sec: 5,
                token: "token",
            },
        )
        .await
        .unwrap_err();
        assert!(error.message().contains("does not accept a model"));
        assert!(check_audio(ProviderKind::OpenAi, b"not audio", "wav").is_err());
        let mut speech_server = Server::new_async().await;
        let truncated = speech_server
            .mock("POST", "/v1/audio/speech")
            .with_status(200)
            .with_body([0xff, 0xfb, 0x90, 0x64, 0])
            .create_async()
            .await;
        assert!(send_speech_request(
            ProviderKind::OpenAi,
            &speech_server.url(),
            ProviderSpeechRequest {
                model: Some("speech-model"),
                text: "hello",
                voice: "coral",
                format: "mp3",
                timeout_in_sec: 5,
                token: "token",
            },
        )
        .await
        .is_err());
        truncated.assert_async().await;
        let mut server = Server::new_async().await;
        server
            .mock("POST", "/v1/stt")
            .with_status(200)
            .with_body(r#"{"text":"  "}"#)
            .create_async()
            .await;
        let error = send_transcription_request(
            ProviderKind::Xai,
            &server.url(),
            ProviderTranscriptionRequest {
                model: "grok-voice-transcribe-2.0",
                filename: "recording.wav",
                audio_bytes: WAV,
                media_type: "audio/wav",
                timeout_in_sec: 5,
                token: "token",
            },
        )
        .await
        .unwrap_err();
        assert!(error.message().contains("no transcript"));
    }
}
