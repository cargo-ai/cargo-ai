use super::{
    media::{checked_body, client, endpoint, json_body, limited_body},
    runtime::{ImageReference, ProviderFacts, ProviderImageResponse},
    ProviderError, ProviderKind,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use reqwest::{redirect::Policy, Client, Url};
use serde_json::{json, Value};
use std::{
    net::{IpAddr, SocketAddr},
    time::{Duration, Instant},
};

const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const MAX_JSON_BYTES: usize = 30 * 1024 * 1024;
const MAX_REFERENCE_BYTES: usize = 10 * 1024 * 1024;

pub(crate) async fn send_image_request(
    provider: ProviderKind,
    url: &str,
    model: &str,
    prompt: &str,
    timeout_in_sec: u64,
    token: &str,
    format: &str,
    reference_images: &[ImageReference],
) -> Result<ProviderImageResponse, ProviderError> {
    if !provider.capabilities().supports_generate_image {
        return Err(ProviderError::invalid_request(
            provider,
            "Image generation is not supported by this provider.",
        ));
    }
    if model.trim().is_empty() || prompt.trim().is_empty() {
        return Err(ProviderError::invalid_request(
            provider,
            "An image model and prompt are required.",
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(timeout_in_sec);
    match provider {
        ProviderKind::OpenAi => {
            return super::openai::send_image_request(
                &url.to_owned(),
                &model.to_owned(),
                prompt,
                timeout_in_sec,
                &token.to_owned(),
                format,
                reference_images,
            )
            .await
        }
        ProviderKind::Ollama => {
            if format != "png" || !reference_images.is_empty() {
                return Err(ProviderError::invalid_request(
                    provider,
                    "Ollama image generation supports PNG without references.",
                ));
            }
            return super::ollama::send_image_request(
                &url.to_owned(),
                &model.to_owned(),
                prompt,
                timeout_in_sec,
                token,
            )
            .await;
        }
        ProviderKind::Gemini => {
            if format != "png" || reference_images.len() > 4 {
                return Err(ProviderError::invalid_request(
                    provider,
                    "Gemini image generation supports PNG output and at most four references.",
                ));
            }
        }
        ProviderKind::Xai => {
            if format != "jpg" && format != "jpeg" || reference_images.len() > 5 {
                return Err(ProviderError::invalid_request(
                    provider,
                    "xAI image generation supports JPEG output and at most five references.",
                ));
            }
        }
        ProviderKind::Mistral => {
            if format != "png" || !reference_images.is_empty() {
                return Err(ProviderError::invalid_request(
                    provider,
                    "Mistral image generation supports PNG without references.",
                ));
            }
        }
        _ => unreachable!(),
    }
    let total = reference_images.iter().try_fold(0usize, |sum, reference| {
        if reference.bytes.len() > MAX_REFERENCE_BYTES {
            None
        } else {
            sum.checked_add(reference.bytes.len())
        }
    });
    if total.is_none_or(|total| total > MAX_IMAGE_BYTES) {
        return Err(ProviderError::invalid_request(
            provider,
            "Reference images exceed the size limit.",
        ));
    }
    let client = client(provider, timeout_in_sec)?;
    let (destination, body) = match provider {
        ProviderKind::Gemini => {
            let mut input = vec![json!({"type":"text","text":prompt})];
            for reference in reference_images {
                if !matches!(reference.media_type.as_str(), "image/png" | "image/jpeg") {
                    return Err(ProviderError::invalid_request(
                        provider,
                        "Gemini references must be PNG or JPEG.",
                    ));
                }
                input.push(json!({"type":"image","data":BASE64_STANDARD.encode(&reference.bytes),"mime_type":reference.media_type}));
            }
            (
                endpoint(provider, url, "/v1beta/interactions")?,
                json!({"model":model,"input":input,"response_format":{"type":"image"},"store":false}),
            )
        }
        ProviderKind::Xai => {
            let (suffix, image) = if reference_images.is_empty() {
                ("/v1/images/generations", None)
            } else {
                let images = reference_images.iter().map(|reference| {
                    if !matches!(reference.media_type.as_str(),"image/png"|"image/jpeg") { return Err(ProviderError::invalid_request(provider,"xAI references must be PNG or JPEG.")); }
                    Ok(json!({"type":"image_url","url":format!("data:{};base64,{}",reference.media_type,BASE64_STANDARD.encode(&reference.bytes))}))
                }).collect::<Result<Vec<_>,_>>()?;
                ("/v1/images/edits", Some(images))
            };
            let mut body =
                json!({"model":model,"prompt":prompt,"n":1,"response_format":"b64_json"});
            if let Some(image) = image {
                if image.len() == 1 {
                    body["image"] = image[0].clone();
                } else {
                    body["images"] = Value::Array(image);
                }
            }
            (endpoint(provider, url, suffix)?, body)
        }
        ProviderKind::Mistral => (
            endpoint(provider, url, "/v1/chat/completions")?,
            json!({"model":model,"messages":[{"role":"user","content":prompt}],"tools":[{"type":"image_generation"}]}),
        ),
        _ => unreachable!(),
    };
    let builder = client.post(destination).json(&body);
    let builder = if provider == ProviderKind::Gemini {
        builder.header("x-goog-api-key", token)
    } else {
        builder.bearer_auth(token)
    };
    let response = builder
        .send()
        .await
        .map_err(|error| ProviderError::from_reqwest(provider, error))?;
    let (body, facts) = checked_body(provider, response, token, MAX_JSON_BYTES).await?;
    let value = json_body(provider, &body).map_err(|error| facts.error(error))?;
    let bytes = match provider {
        ProviderKind::Gemini => {
            let images = value["steps"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|step| step["type"] == "model_output")
                .flat_map(|step| step["content"].as_array().into_iter().flatten())
                .filter(|content| content["type"] == "image")
                .collect::<Vec<_>>();
            if images.len() != 1 {
                return Err(facts.error(ProviderError::invalid_response(
                    provider,
                    "Gemini did not return exactly one image.",
                )));
            }
            if images[0]["mime_type"]
                .as_str()
                .is_some_and(|mime| mime != "image/png")
            {
                return Err(facts.error(ProviderError::invalid_response(
                    provider,
                    "Gemini returned an unexpected image MIME type.",
                )));
            }
            decode_image(provider, images[0]["data"].as_str(), &facts)?
        }
        ProviderKind::Xai => {
            let items = value["data"].as_array().ok_or_else(|| {
                facts.error(ProviderError::invalid_response(
                    provider,
                    "xAI returned no image data.",
                ))
            })?;
            if items.len() != 1 {
                return Err(facts.error(ProviderError::invalid_response(
                    provider,
                    "xAI did not return exactly one image.",
                )));
            }
            decode_image(provider, items[0]["b64_json"].as_str(), &facts)?
        }
        ProviderKind::Mistral => {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(facts.error(ProviderError::invalid_response(
                    provider,
                    "Image retrieval deadline expired.",
                )));
            }
            mistral_image(&value, url, remaining, token, &facts).await?
        }
        _ => unreachable!(),
    };
    validate_image(provider, &bytes, format).map_err(|error| facts.error(error))?;
    Ok(ProviderImageResponse {
        resolved_model: facts.resolved_model,
        provider_request_id: facts.provider_request_id,
        finish_reason: None,
        bytes,
        usage: facts.usage,
    })
}

fn decode_image(
    provider: ProviderKind,
    encoded: Option<&str>,
    facts: &ProviderFacts,
) -> Result<Vec<u8>, ProviderError> {
    let encoded = encoded.filter(|data| !data.is_empty()).ok_or_else(|| {
        facts.error(ProviderError::invalid_response(
            provider,
            "Provider returned no encoded image.",
        ))
    })?;
    if encoded.len() > ((MAX_IMAGE_BYTES + 2) / 3) * 4 + 16 {
        return Err(facts.error(ProviderError::invalid_response(
            provider,
            "Encoded image exceeded the size limit.",
        )));
    }
    BASE64_STANDARD.decode(encoded).map_err(|_| {
        facts.error(ProviderError::invalid_response(
            provider,
            "Provider returned invalid base64 image.",
        ))
    })
}

fn validate_image(provider: ProviderKind, bytes: &[u8], format: &str) -> Result<(), ProviderError> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(ProviderError::invalid_response(
            provider,
            "Generated image exceeded the size limit.",
        ));
    }
    let valid = match format {
        "png" => valid_png(bytes),
        "jpg" | "jpeg" => valid_jpeg(bytes),
        _ => {
            return Err(ProviderError::invalid_request(
                provider,
                "Unsupported image output format.",
            ))
        }
    };
    if valid {
        Ok(())
    } else {
        Err(ProviderError::invalid_response(
            provider,
            "Generated image did not match the requested file format.",
        ))
    }
}

fn valid_png(bytes: &[u8]) -> bool {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return false;
    }
    let mut offset = 8usize;
    let mut saw_header = false;
    let mut saw_data = false;
    while offset.checked_add(12).is_some_and(|end| end <= bytes.len()) {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let Some(end) = offset.checked_add(12).and_then(|n| n.checked_add(length)) else {
            return false;
        };
        if end > bytes.len() {
            return false;
        }
        let kind = &bytes[offset + 4..offset + 8];
        let data = &bytes[offset + 8..offset + 8 + length];
        if !saw_header {
            if kind != b"IHDR" || length != 13 {
                return false;
            }
            let width = u32::from_be_bytes(data[0..4].try_into().unwrap());
            let height = u32::from_be_bytes(data[4..8].try_into().unwrap());
            if width == 0 || height == 0 || data[12] > 1 {
                return false;
            }
            saw_header = true;
        } else if kind == b"IHDR" {
            return false;
        }
        if kind == b"IDAT" {
            saw_data |= length > 0;
        }
        if kind == b"IEND" {
            return saw_header && saw_data && length == 0 && end == bytes.len();
        }
        offset = end;
    }
    false
}

fn valid_jpeg(bytes: &[u8]) -> bool {
    if bytes.len() < 16 || !bytes.starts_with(b"\xff\xd8") || !bytes.ends_with(b"\xff\xd9") {
        return false;
    }
    let mut offset = 2usize;
    let mut saw_frame = false;
    while offset + 4 <= bytes.len() {
        if bytes[offset] != 0xff {
            return false;
        }
        while offset < bytes.len() && bytes[offset] == 0xff {
            offset += 1;
        }
        if offset + 2 >= bytes.len() {
            return false;
        }
        let marker = bytes[offset];
        offset += 1;
        if matches!(marker, 0x01 | 0xd0..=0xd9) {
            return false;
        }
        let length = u16::from_be_bytes(bytes[offset..offset + 2].try_into().unwrap()) as usize;
        if length < 2 {
            return false;
        }
        let Some(end) = offset.checked_add(length) else {
            return false;
        };
        if end > bytes.len() - 2 {
            return false;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            if length < 8 || bytes[offset + 3..offset + 7].iter().all(|b| *b == 0) {
                return false;
            }
            let height = u16::from_be_bytes(bytes[offset + 3..offset + 5].try_into().unwrap());
            let width = u16::from_be_bytes(bytes[offset + 5..offset + 7].try_into().unwrap());
            if height == 0 || width == 0 {
                return false;
            }
            saw_frame = true;
        }
        if marker == 0xda {
            return saw_frame && end < bytes.len() - 2;
        }
        offset = end;
    }
    false
}

async fn mistral_image(
    value: &Value,
    base_url: &str,
    timeout: Duration,
    token: &str,
    facts: &ProviderFacts,
) -> Result<Vec<u8>, ProviderError> {
    let provider = ProviderKind::Mistral;
    let choices = value["choices"].as_array().ok_or_else(|| {
        facts.error(ProviderError::invalid_response(
            provider,
            "Mistral returned no image choice.",
        ))
    })?;
    if choices.len() != 1 {
        return Err(facts.error(ProviderError::invalid_response(
            provider,
            "Mistral did not return exactly one choice.",
        )));
    }
    let choice = &choices[0];
    let messages = choice["messages"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![choice["message"].clone()]);
    let mut ids = Vec::new();
    let mut urls = Vec::new();
    for message in messages {
        if let Some(contents) = message["content"].as_array() {
            for content in contents {
                if content["type"] == "tool_file" {
                    if let Some(id) = content["file_id"].as_str() {
                        ids.push(id.to_owned());
                    }
                } else if content["type"] == "image_url" {
                    let url = content["image_url"]
                        .as_str()
                        .or_else(|| content["image_url"]["url"].as_str());
                    if let Some(url) = url {
                        urls.push(url.to_owned());
                    }
                }
            }
        } else if let Some(text) = message["content"].as_str() {
            for segment in text.split("[Image: ").skip(1) {
                if let Some((url, _)) = segment.split_once(']') {
                    urls.push(url.to_owned());
                }
            }
        }
    }
    if ids.len() + urls.len() != 1 {
        return Err(facts.error(ProviderError::invalid_response(
            provider,
            "Mistral did not return exactly one image file.",
        )));
    }
    if let Some(id) = ids.first() {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(facts.error(ProviderError::invalid_response(
                provider,
                "Mistral returned an invalid image file ID.",
            )));
        }
        let base = endpoint(provider, base_url, &format!("/v1/files/{id}/content"))?;
        if base.scheme() != "https"
            && !(cfg!(test)
                && base
                    .host_str()
                    .is_some_and(|host| host == "127.0.0.1" || host == "localhost"))
        {
            return Err(facts.error(ProviderError::invalid_request(
                provider,
                "Mistral file retrieval requires HTTPS API origin.",
            )));
        }
        let response = Client::builder()
            .timeout(timeout)
            .redirect(Policy::none())
            .build()
            .map_err(|error| ProviderError::from_reqwest(provider, error))?
            .get(base)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| ProviderError::from_reqwest(provider, error))?;
        let (bytes, _) = checked_body(provider, response, token, MAX_IMAGE_BYTES).await?;
        return Ok(bytes);
    }
    fetch_public_image(&urls[0], timeout)
        .await
        .map_err(|error| facts.error(error))
}

async fn fetch_public_image(url: &str, timeout: Duration) -> Result<Vec<u8>, ProviderError> {
    let provider = ProviderKind::Mistral;
    let parsed = Url::parse(url).map_err(|_| {
        ProviderError::invalid_response(provider, "Mistral returned an invalid image URL.")
    })?;
    if parsed.scheme() != "https" || parsed.username() != "" || parsed.password().is_some() {
        return Err(ProviderError::invalid_response(
            provider,
            "Mistral image URL must use HTTPS without user information.",
        ));
    }
    let host = parsed.host_str().ok_or_else(|| {
        ProviderError::invalid_response(provider, "Mistral image URL has no host.")
    })?;
    let port = parsed.port_or_known_default().unwrap_or(443);
    let literal_ip = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .ok();
    let addresses = if let Some(ip) = literal_ip {
        vec![SocketAddr::new(ip, port)]
    } else {
        tokio::time::timeout(timeout, tokio::net::lookup_host((host, port)))
            .await
            .map_err(|_| {
                ProviderError::invalid_response(provider, "Image URL resolution timed out.")
            })?
            .map_err(|_| {
                ProviderError::invalid_response(provider, "Image URL could not be resolved.")
            })?
            .collect::<Vec<SocketAddr>>()
    };
    if addresses.is_empty() || addresses.iter().any(|address| !public_ip(address.ip())) {
        return Err(ProviderError::invalid_response(
            provider,
            "Mistral image URL resolved outside public network addresses.",
        ));
    }
    let mut builder = Client::builder()
        .timeout(timeout)
        .redirect(Policy::none())
        .no_proxy();
    if literal_ip.is_none() {
        builder = builder.resolve(host, addresses[0]);
    }
    let client = builder
        .build()
        .map_err(|error| ProviderError::from_reqwest(provider, error))?;
    let response = client
        .get(parsed)
        .send()
        .await
        .map_err(|error| ProviderError::from_reqwest(provider, error))?;
    if !response.status().is_success() {
        return Err(ProviderError::invalid_response(
            provider,
            "Mistral image download failed.",
        ));
    }
    limited_body(provider, response, MAX_IMAGE_BYTES).await
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_multicast()
                || ip.is_unspecified()
                || octets[0] == 0
                || octets[0] >= 224
                || octets[0] == 100 && (64..=127).contains(&octets[1])
                || octets[0] == 192 && octets[1] == 0 && octets[2] == 0
                || octets[0] == 192 && octets[1] == 0 && octets[2] == 2
                || octets[0] == 192 && octets[1] == 88 && octets[2] == 99
                || octets[0] == 198 && matches!(octets[1], 18 | 19)
                || octets[0] == 198 && octets[1] == 51 && octets[2] == 100
                || octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(v4));
            }
            let seg = ip.segments();
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || seg[0] & 0xe000 != 0x2000
                || seg[0] == 0x2001 && (seg[1] & 0xfe00 == 0 || seg[1] == 0xdb8)
                || seg[0] == 0x2002
                || seg[0] == 0x3fff && seg[1] & 0xf000 == 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    #[test]
    fn public_image_addresses_exclude_special_purpose_ranges() {
        for address in [
            "192.88.99.1",
            "198.18.0.1",
            "198.19.255.254",
            "198.51.100.1",
            "2001:10::1",
            "2001:20::1",
            "2001:30::1",
            "2001:db8::1",
            "2002::1",
            "3fff::1",
            "::ffff:198.51.100.1",
        ] {
            assert!(!public_ip(address.parse().unwrap()), "{address}");
        }
        for address in [
            "198.51.99.1",
            "198.51.101.1",
            "2001:200::1",
            "2001:db9::1",
            "3fff:1000::1",
        ] {
            assert!(public_ip(address.parse().unwrap()), "{address}");
        }
    }

    const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAAAXNSR0IArs4c6QAAAERlWElmTU0AKgAAAAgAAYdpAAQAAAABAAAAGgAAAAAAA6ABAAMAAAABAAEAAKACAAQAAAABAAAAAaADAAQAAAABAAAAAQAAAAD5Ip3+AAAADElEQVQIHWNISXcGAAJBAQ/t+dCDAAAAAElFTkSuQmCC";
    const JPEG_B64: &str = "/9j/4AAQSkZJRgABAQAASABIAAD/4QBMRXhpZgAATU0AKgAAAAgAAYdpAAQAAAABAAAAGgAAAAAAA6ABAAMAAAABAAEAAKACAAQAAAABAAAAAaADAAQAAAABAAAAAQAAAAD/7QA4UGhvdG9zaG9wIDMuMAA4QklNBAQAAAAAAAA4QklNBCUAAAAAABDUHYzZjwCyBOmACZjs+EJ+/8AAEQgAAQABAwEiAAIRAQMRAf/EAB8AAAEFAQEBAQEBAAAAAAAAAAABAgMEBQYHCAkKC//EALUQAAIBAwMCBAMFBQQEAAABfQECAwAEEQUSITFBBhNRYQcicRQygZGhCCNCscEVUtHwJDNicoIJChYXGBkaJSYnKCkqNDU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6g4SFhoeIiYqSk5SVlpeYmZqio6Slpqeoqaqys7S1tre4ubrCw8TFxsfIycrS09TV1tfY2drh4uPk5ebn6Onq8fLz9PX29/j5+v/EAB8BAAMBAQEBAQEBAQEAAAAAAAABAgMEBQYHCAkKC//EALURAAIBAgQEAwQHBQQEAAECdwABAgMRBAUhMQYSQVEHYXETIjKBCBRCkaGxwQkjM1LwFWJy0QoWJDThJfEXGBkaJicoKSo1Njc4OTpDREVGR0hJSlNUVVZXWFlaY2RlZmdoaWpzdHV2d3h5eoKDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uLj5OXm5+jp6vLz9PX29/j5+v/bAEMAAgICAgICAwICAwUDAwMFBgUFBQUGCAYGBgYGCAoICAgICAgKCgoKCgoKCgwMDAwMDA4ODg4ODw8PDw8PDw8PD//bAEMBAgICBAQEBwQEBxALCQsQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEP/dAAQAAf/aAAwDAQACEQMRAD8A8Hooor87ND//2Q==";

    #[tokio::test]
    async fn native_image_generation_decodes_one_expected_result() {
        let png = BASE64_STANDARD.decode(PNG_B64).unwrap();
        let jpeg = BASE64_STANDARD.decode(JPEG_B64).unwrap();
        for provider in [
            ProviderKind::Gemini,
            ProviderKind::Xai,
            ProviderKind::Mistral,
        ] {
            let mut server = Server::new_async().await;
            let (path, body, format, expected) = match provider {
                ProviderKind::Gemini => (
                    "/v1beta/interactions",
                    json!({"steps":[{"type":"model_output","content":[{"type":"image","mime_type":"image/png","data":BASE64_STANDARD.encode(&png)}]}]}),
                    "png",
                    png.as_slice(),
                ),
                ProviderKind::Xai => (
                    "/v1/images/generations",
                    json!({"data":[{"b64_json":BASE64_STANDARD.encode(&jpeg)}]}),
                    "jpg",
                    jpeg.as_slice(),
                ),
                ProviderKind::Mistral => (
                    "/v1/chat/completions",
                    json!({"choices":[{"messages":[{"content":[{"type":"tool_file","file_id":"generated123"}]}]}]}),
                    "png",
                    png.as_slice(),
                ),
                _ => unreachable!(),
            };
            let expected_request = if provider == ProviderKind::Gemini {
                Matcher::Json(
                    json!({"model":"image-model","input":[{"type":"text","text":"draw a square"}],"response_format":{"type":"image"},"store":false}),
                )
            } else {
                Matcher::Regex("draw a square".into())
            };
            let fixture = server
                .mock("POST", path)
                .match_body(expected_request)
                .with_status(200)
                .with_body(body.to_string())
                .create_async()
                .await;
            let fetched = if provider == ProviderKind::Mistral {
                Some(
                    server
                        .mock("GET", "/v1/files/generated123/content")
                        .match_header("authorization", "Bearer secret-token")
                        .with_status(200)
                        .with_body(png.clone())
                        .create_async()
                        .await,
                )
            } else {
                None
            };
            let response = send_image_request(
                provider,
                &server.url(),
                "image-model",
                "draw a square",
                5,
                "secret-token",
                format,
                &[],
            )
            .await
            .unwrap();
            assert_eq!(response.bytes, expected);
            fixture.assert_async().await;
            if let Some(fetched) = fetched {
                fetched.assert_async().await;
            }
        }
    }

    #[tokio::test]
    async fn references_and_unsafe_results_fail_before_writing() {
        let png = BASE64_STANDARD.decode(PNG_B64).unwrap();
        let jpeg = BASE64_STANDARD.decode(JPEG_B64).unwrap();
        let reference = ImageReference {
            source: "fixture.png".into(),
            filename: "fixture.png".into(),
            media_type: "image/png".into(),
            data_url: format!("data:image/png;base64,{}", BASE64_STANDARD.encode(&png)),
            bytes: png,
        };
        let mut server = Server::new_async().await;
        let fixture = server
            .mock("POST", "/v1/images/edits")
            .match_body(Matcher::Regex("data:image/png;base64".into()))
            .with_status(200)
            .with_body(json!({"data":[{"b64_json":BASE64_STANDARD.encode(&jpeg)}]}).to_string())
            .create_async()
            .await;
        let response = send_image_request(
            ProviderKind::Xai,
            &server.url(),
            "image-model",
            "edit",
            5,
            "token",
            "jpg",
            &[reference.clone()],
        )
        .await
        .unwrap();
        assert_eq!(response.bytes, jpeg);
        fixture.assert_async().await;
        let error = send_image_request(
            ProviderKind::Mistral,
            &server.url(),
            "image-model",
            "edit",
            5,
            "token",
            "png",
            &[reference],
        )
        .await
        .unwrap_err();
        assert!(error.message().contains("without references"));
        assert!(!public_ip("127.0.0.1".parse().unwrap()));
        assert!(!public_ip("169.254.2.1".parse().unwrap()));
        assert!(!public_ip("::1".parse().unwrap()));
        assert!(public_ip("8.8.8.8".parse().unwrap()));
    }

    #[tokio::test]
    async fn malformed_images_and_unsafe_retrieval_are_rejected() {
        let facts = ProviderFacts::default();
        assert!(validate_image(ProviderKind::Gemini, b"\x89PNG\r\n\x1a\n", "png").is_err());
        assert!(validate_image(ProviderKind::Xai, b"\xff\xd8\xff", "jpg").is_err());
        let oversized = "A".repeat(((MAX_IMAGE_BYTES + 2) / 3) * 4 + 17);
        assert!(decode_image(ProviderKind::Xai, Some(&oversized), &facts).is_err());
        for value in [
            json!({"choices":[{"messages":[{"content":[]}]}]}),
            json!({"choices":[{"messages":[{"content":[{"type":"tool_file","file_id":"a"},{"type":"tool_file","file_id":"b"}]}]}]}),
        ] {
            let error = mistral_image(
                &value,
                "https://api.mistral.ai/v1/chat/completions",
                Duration::from_secs(1),
                "token",
                &facts,
            )
            .await
            .unwrap_err();
            assert!(error.message().contains("exactly one image file"));
        }
        for ip in [
            "10.0.0.1",
            "127.0.0.1",
            "169.254.2.1",
            "::1",
            "fc00::1",
            "::ffff:127.0.0.1",
        ] {
            let url = if ip.contains(':') {
                format!("https://[{ip}]/private?sig=secret-query")
            } else {
                format!("https://{ip}/private?sig=secret-query")
            };
            let error = fetch_public_image(&url, Duration::from_secs(1))
                .await
                .unwrap_err();
            assert!(!error.message().contains("secret-query"));
        }
    }

    #[tokio::test]
    async fn file_retrieval_does_not_follow_redirect_or_expose_token() {
        let mut server = Server::new_async().await;
        server.mock("POST","/v1/chat/completions")
            .with_status(200).with_body(json!({"choices":[{"messages":[{"content":[{"type":"tool_file","file_id":"generated123"}]}]}]}).to_string())
            .create_async().await;
        let fetched = server
            .mock("GET", "/v1/files/generated123/content")
            .match_header("authorization", "Bearer secret-token")
            .with_status(302)
            .with_header("location", "https://127.0.0.1/private?sig=secret-query")
            .with_body(json!({"error":{"message":"secret-token reflected"}}).to_string())
            .create_async()
            .await;
        let error = send_image_request(
            ProviderKind::Mistral,
            &server.url(),
            "image-model",
            "draw",
            5,
            "secret-token",
            "png",
            &[],
        )
        .await
        .unwrap_err();
        fetched.assert_async().await;
        assert!(!error.message().contains("secret-token"));
        assert!(!error.message().contains("secret-query"));
    }
}
