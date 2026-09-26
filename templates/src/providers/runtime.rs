use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::{Deserialize, Serialize}; // Data format (e.g.,JSON, TOML) (de)serialization
use serde_json;
use std::{collections::BTreeMap, fs, io::Read, path::Path};

const SUPPORTED_FILE_EXTENSIONS_MESSAGE: &str =
    "pdf, docx, csv, xla, xlb, xlc, xlm, xls, xlsx, xlt, xlw, tsv, iif, doc, dot, odt, rtf, pot, ppa, pps, ppt, pptx, pwz, wiz";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ContentPart {
    Text(String),
    Image { data_url: String },
    File { filename: String, file_data: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ImageReference {
    pub(crate) source: String,
    pub(crate) filename: String,
    pub(crate) media_type: String,
    pub(crate) data_url: String,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) trait ValidatedResponse {
    // A complete raw check avoids evaluating the same schema again after parsing.
    const RAW_VALIDATION_COMPLETE: bool = false;

    fn validate_raw_response(_value: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }

    fn validate_response(&self) -> Result<(), String>;
}

impl ValidatedResponse for serde_json::Value {
    fn validate_response(&self) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ProviderUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) total_tokens_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) input_token_details: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) output_token_details: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProviderTextResponse {
    pub(crate) provider_request_id: Option<String>,
    pub(crate) finish_reason: Option<String>,
    pub(crate) resolved_model: Option<String>,
    pub(crate) text: String,
    pub(crate) usage: Option<ProviderUsage>,
}

pub(crate) struct ProviderTextRequest<'a> {
    pub(crate) rubric_enabled: bool,
    pub(crate) model: &'a str,
    pub(crate) content_parts: &'a [ContentPart],
    pub(crate) timeout_in_sec: u64,
    pub(crate) token: &'a str,
    pub(crate) response_schema: &'a serde_json::Value,
    pub(crate) max_output_tokens: Option<u32>,
    pub(crate) temperature: Option<f64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProviderImageResponse {
    pub(crate) resolved_model: Option<String>,
    pub(crate) provider_request_id: Option<String>,
    pub(crate) finish_reason: Option<String>,
    pub(crate) bytes: Vec<u8>,
    pub(crate) usage: Option<ProviderUsage>,
}

#[derive(Debug)]
pub struct Cargo<T: for<'de> Deserialize<'de> + Serialize + Clone + ValidatedResponse> {
    inputs: Vec<ContentPart>,
    context: String,
    #[cfg_attr(not(test), allow(dead_code))]
    response: Option<T>,
} // TODO: Hints

// We use `for<'de>` to tell the compiler how long any borrowed data inside T
// must stay valid during deserialization. This annotation only guides the compiler;
// it does not tie that lifetime to the entire struct.
impl<T: for<'de> Deserialize<'de> + Serialize + Clone + ValidatedResponse> Cargo<T> {
    pub fn new(inputs: Vec<ContentPart>, context: String) -> Self {
        Cargo {
            inputs,
            context,
            response: None,
        }
    }

    pub fn content_parts(&self) -> Vec<ContentPart> {
        let mut content_parts = Vec::with_capacity(self.inputs.len() + 1);

        if !self.context.trim().is_empty() {
            content_parts.push(ContentPart::Text(format!(
                "For context {} \n",
                self.context
            )));
        }

        content_parts.extend(self.inputs.iter().cloned());
        content_parts
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_response(&mut self, response: String) -> bool {
        let parsed = serde_json::from_str::<serde_json::Value>(&response)
            .ok()
            .filter(|value| T::validate_raw_response(value).is_ok())
            .and_then(|value| serde_json::from_value::<T>(value).ok());
        match parsed {
            Some(response) => match if T::RAW_VALIDATION_COMPLETE {
                Ok(())
            } else {
                response.validate_response()
            } {
                Ok(()) => {
                    self.response = Some(response);
                    true
                }
                Err(_) => {
                    self.response = None;
                    false
                }
            },
            None => {
                // Keep state deterministic: failed parse must not retain stale success output.
                self.response = None;
                false
            }
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn get_response(&self) -> Option<T> {
        self.response.clone()
    }
}

pub(crate) async fn resolve_inputs(inputs: &[crate::Input]) -> Result<Vec<ContentPart>, String> {
    let mut url_positions = Vec::new();
    let mut url_values = Vec::new();

    for (index, input) in inputs.iter().enumerate() {
        if input.kind == crate::InputKind::Url {
            let url = require_input_value(input)?;
            url_positions.push(index);
            url_values.push(url);
        }
    }

    let fetched_url_text = if url_values.is_empty() {
        Vec::new()
    } else {
        crate::web_resources::fetch_resources_parallel(&url_values).await?
    };

    let fetched_by_index = url_positions
        .into_iter()
        .zip(fetched_url_text.into_iter())
        .collect::<BTreeMap<usize, String>>();

    let mut resolved = Vec::with_capacity(inputs.len());
    for (index, input) in inputs.iter().enumerate() {
        match input.kind {
            crate::InputKind::Text => {
                resolved.push(ContentPart::Text(require_input_value(input)?.to_string()))
            }
            crate::InputKind::Url => {
                let url = require_input_value(input)?;
                let content = fetched_by_index.get(&index).ok_or_else(|| {
                    format!("Resolved URL input missing fetched content for '{}'.", url)
                })?;
                resolved.push(ContentPart::Text(format!(
                    "Web resource from {}:\n{}",
                    url,
                    content.trim()
                )));
            }
            crate::InputKind::Image => resolved.push(ContentPart::Image {
                data_url: load_image_data_url(require_input_value(input)?)?,
            }),
            crate::InputKind::File => {
                resolved.push(load_supported_file_content(require_input_value(input)?)?)
            }
        }
    }

    Ok(resolved)
}

fn require_input_value(input: &crate::Input) -> Result<&str, String> {
    input.value.as_deref().ok_or_else(|| {
        if let Some(name) = input.name.as_deref() {
            format!(
                "Named input '{}' ({}) is required for this invocation but has no value.",
                name,
                input.kind_label()
            )
        } else {
            format!(
                "{} input is required for this invocation but has no value.",
                input.kind_label()
            )
        }
    })
}

fn load_image_data_url(path: &str) -> Result<String, String> {
    let image_path = Path::new(path);
    let image_bytes = fs::read(image_path).map_err(|error| {
        format!(
            "Failed to read image input '{}': {error}",
            image_path.display()
        )
    })?;
    let media_type = image_media_type(image_path)?;
    let encoded = BASE64_STANDARD.encode(image_bytes);
    Ok(format!("data:{media_type};base64,{encoded}"))
}

pub(crate) fn load_image_reference(path: &str) -> Result<ImageReference, String> {
    let image_path = Path::new(path);
    let image_bytes = fs::read(image_path).map_err(|error| {
        format!(
            "Failed to read reference image '{}': {error}",
            image_path.display()
        )
    })?;
    image_reference_from_bytes(image_path, image_bytes)
}

pub(crate) fn load_image_reference_with_limit(
    path: &str,
    max_bytes: usize,
) -> Result<ImageReference, String> {
    let image_path = Path::new(path);
    let before_open = fs::metadata(image_path).map_err(|error| {
        format!(
            "Failed to inspect reference image '{}': {error}",
            image_path.display()
        )
    })?;
    if !before_open.is_file() || before_open.len() > max_bytes as u64 {
        return Err(format!(
            "Reference image '{}' must be a regular file no larger than {max_bytes} bytes.",
            image_path.display()
        ));
    }
    let mut file = fs::File::open(image_path).map_err(|error| {
        format!(
            "Failed to open reference image '{}': {error}",
            image_path.display()
        )
    })?;
    let metadata = file.metadata().map_err(|error| {
        format!(
            "Failed to inspect reference image '{}': {error}",
            image_path.display()
        )
    })?;
    if !metadata.is_file() || metadata.len() > max_bytes as u64 {
        return Err(format!(
            "Reference image '{}' must be a regular file no larger than {max_bytes} bytes.",
            image_path.display()
        ));
    }
    let mut image_bytes = Vec::new();
    (&mut file)
        .take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut image_bytes)
        .map_err(|error| {
            format!(
                "Failed to read reference image '{}': {error}",
                image_path.display()
            )
        })?;
    if image_bytes.len() > max_bytes {
        return Err(format!(
            "Reference image '{}' exceeds the {max_bytes}-byte limit.",
            image_path.display()
        ));
    }
    image_reference_from_bytes(image_path, image_bytes)
}

fn image_reference_from_bytes(
    image_path: &Path,
    image_bytes: Vec<u8>,
) -> Result<ImageReference, String> {
    let media_type = image_media_type(image_path)?;
    let encoded = BASE64_STANDARD.encode(&image_bytes);
    let filename = image_path
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            format!(
                "Reference image '{}' must include a filename.",
                image_path.display()
            )
        })?;

    Ok(ImageReference {
        source: image_path.display().to_string(),
        filename,
        media_type: media_type.to_string(),
        data_url: format!("data:{media_type};base64,{encoded}"),
        bytes: image_bytes,
    })
}

fn load_supported_file_content(path: &str) -> Result<ContentPart, String> {
    let file_path = Path::new(path);
    let media_type = supported_file_media_type(file_path)?;

    let metadata = fs::metadata(file_path).map_err(|error| {
        format!(
            "Failed to inspect file input '{}': {error}",
            file_path.display()
        )
    })?;

    if !metadata.is_file() {
        return Err(format!(
            "File input '{}' must point to a regular file.",
            file_path.display()
        ));
    }

    let file_bytes = fs::read(file_path).map_err(|error| {
        format!(
            "Failed to read file input '{}': {error}",
            file_path.display()
        )
    })?;

    let filename = file_path
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            format!(
                "File input '{}' must include a filename.",
                file_path.display()
            )
        })?;

    Ok(ContentPart::File {
        filename,
        file_data: file_data_url(media_type, &file_bytes),
    })
}

fn file_data_url(media_type: &str, file_bytes: &[u8]) -> String {
    let encoded = BASE64_STANDARD.encode(file_bytes);
    format!("data:{media_type};base64,{encoded}")
}

fn supported_file_media_type(path: &Path) -> Result<&'static str, String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .ok_or_else(|| {
            format!(
                "File input '{}' must include a supported extension. Supported: {SUPPORTED_FILE_EXTENSIONS_MESSAGE}.",
                path.display()
            )
        })?;

    match extension.as_str() {
        "pdf" => Ok("application/pdf"),
        "doc" | "dot" => Ok("application/msword"),
        "docx" => Ok("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        "odt" => Ok("application/vnd.oasis.opendocument.text"),
        "rtf" => Ok("application/rtf"),
        "csv" => Ok("text/csv"),
        "tsv" => Ok("text/tsv"),
        "iif" => Ok("text/x-iif"),
        "xla" | "xlb" | "xlc" | "xlm" | "xls" | "xlt" | "xlw" => {
            Ok("application/vnd.ms-excel")
        }
        "xlsx" => Ok("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        "pot" | "ppa" | "pps" | "ppt" | "pwz" | "wiz" => Ok("application/vnd.ms-powerpoint"),
        "pptx" => Ok("application/vnd.openxmlformats-officedocument.presentationml.presentation"),
        other => Err(format!(
            "File input '{}' uses unsupported extension '{}'. Supported: {SUPPORTED_FILE_EXTENSIONS_MESSAGE}.",
            path.display(),
            other
        )),
    }
}

fn image_media_type(path: &Path) -> Result<&'static str, String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .ok_or_else(|| {
            format!(
                "Image input '{}' must include a supported file extension.",
                path.display()
            )
        })?;

    match extension.as_str() {
        "png" => Ok("image/png"),
        "jpg" | "jpeg" => Ok("image/jpeg"),
        "gif" => Ok("image/gif"),
        "webp" => Ok("image/webp"),
        other => Err(format!(
            "Image input '{}' uses unsupported extension '{}'. Supported: png, jpg, jpeg, gif, webp.",
            path.display(),
            other
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{Cargo, ContentPart, ValidatedResponse};
    use base64::Engine as _;
    use serde::{Deserialize, Serialize};
    use std::path::Path;

    #[test]
    fn bounded_image_reference_rejects_bytes_before_encoding() {
        let path =
            std::env::temp_dir().join(format!("cargo-ai-reference-{}.png", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"12345").unwrap();
        let path = path.to_str().unwrap();
        let error = super::load_image_reference_with_limit(path, 4).unwrap_err();
        assert!(error.contains("no larger than 4 bytes"));
        let reference = super::load_image_reference_with_limit(path, 5).unwrap();
        assert_eq!(reference.bytes, b"12345");
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn bounded_image_reference_rejects_fifo_without_blocking() {
        use std::sync::mpsc;
        use std::time::Duration;

        let path =
            std::env::temp_dir().join(format!("cargo-ai-reference-{}.png", uuid::Uuid::new_v4()));
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("mkfifo must be available on Unix");
        assert!(status.success());

        let (sender, receiver) = mpsc::channel();
        let worker_path = path.clone();
        std::thread::spawn(move || {
            let result = super::load_image_reference_with_limit(worker_path.to_str().unwrap(), 4);
            let _ = sender.send(result);
        });
        let result = receiver.recv_timeout(Duration::from_secs(2));
        std::fs::remove_file(&path).unwrap();
        let error = result
            .expect("FIFO rejection must finish before the timeout")
            .unwrap_err();
        assert!(error.contains("must be a regular file"));
    }

    #[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
    struct SampleOutput {
        answer: i32,
    }

    impl ValidatedResponse for SampleOutput {
        fn validate_response(&self) -> Result<(), String> {
            Ok(())
        }
    }

    #[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
    struct RejectingOutput {
        answer: i32,
    }

    impl ValidatedResponse for RejectingOutput {
        fn validate_response(&self) -> Result<(), String> {
            Err("rejected".to_string())
        }
    }

    #[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
    struct RawValidatedOutput {
        answer: i32,
    }

    impl ValidatedResponse for RawValidatedOutput {
        const RAW_VALIDATION_COMPLETE: bool = true;

        fn validate_raw_response(value: &serde_json::Value) -> Result<(), String> {
            crate::definition_validation::validate_model_output(
                value,
                &serde_json::json!({
                    "type": "object",
                    "properties": { "answer": { "type": "integer", "minimum": 1 } }
                }),
            )
            .map_err(|error| error.to_string())
        }

        fn validate_response(&self) -> Result<(), String> {
            panic!("a complete raw check must not repeat typed validation")
        }
    }

    #[test]
    fn content_parts_prefix_context_and_preserve_input_order() {
        let cargo = Cargo::<SampleOutput>::new(
            vec![
                ContentPart::Text("first".to_string()),
                ContentPart::Text("second".to_string()),
            ],
            "context".to_string(),
        );

        assert_eq!(
            cargo.content_parts(),
            vec![
                ContentPart::Text("For context context \n".to_string()),
                ContentPart::Text("first".to_string()),
                ContentPart::Text("second".to_string()),
            ]
        );
    }

    #[test]
    fn set_response_stores_successful_parse() {
        let mut cargo = Cargo::<SampleOutput>::new(vec![], "context".to_string());

        assert!(cargo.set_response(r#"{"answer":4}"#.to_string()));
        assert_eq!(cargo.get_response(), Some(SampleOutput { answer: 4 }));
    }

    #[test]
    fn set_response_failure_clears_previous_response() {
        let mut cargo = Cargo::<SampleOutput>::new(vec![], "context".to_string());

        assert!(cargo.set_response(r#"{"answer":4}"#.to_string()));
        assert_eq!(cargo.get_response(), Some(SampleOutput { answer: 4 }));

        assert!(!cargo.set_response("not-json".to_string()));
        assert_eq!(cargo.get_response(), None);
    }

    #[test]
    fn set_response_rejects_parsed_output_that_fails_local_validation() {
        let mut cargo = Cargo::<RejectingOutput>::new(vec![], "context".to_string());

        assert!(!cargo.set_response(r#"{"answer":4}"#.to_string()));
        assert_eq!(cargo.get_response(), None);
    }

    #[test]
    fn raw_validation_rejects_fields_before_deserialization_and_clears_success() {
        let mut cargo = Cargo::<RawValidatedOutput>::new(vec![], String::new());
        for invalid in [
            r#"{"answer":4,"permission":"run more commands"}"#,
            r#"{"answer":0}"#,
            r#"{"answer":"4"}"#,
            r#"{}"#,
        ] {
            assert!(cargo.set_response(r#"{"answer":4}"#.to_string()));
            assert_eq!(cargo.get_response(), Some(RawValidatedOutput { answer: 4 }));
            assert!(!cargo.set_response(invalid.to_string()), "{invalid}");
            assert_eq!(cargo.get_response(), None);
        }
    }

    #[test]
    fn detects_when_content_parts_include_images() {
        let text_only = [ContentPart::Text("x".to_string())];
        assert!(!text_only
            .iter()
            .any(|part| matches!(part, ContentPart::Image { .. })));

        let with_image = [
            ContentPart::Text("x".to_string()),
            ContentPart::Image {
                data_url: "data:image/png;base64,abc".to_string(),
            },
        ];
        assert!(with_image
            .iter()
            .any(|part| matches!(part, ContentPart::Image { .. })));
    }

    #[test]
    fn load_supported_file_content_reads_pdf_bytes() {
        let temp_path =
            std::env::temp_dir().join(format!("cai2036-runtime-{}.pdf", std::process::id()));
        std::fs::write(&temp_path, b"%PDF-1.4\n%mock\n").expect("pdf fixture should write");

        let content = super::load_supported_file_content(temp_path.to_str().expect("utf8 path"))
            .expect("pdf content should load");

        let _ = std::fs::remove_file(&temp_path);

        assert_eq!(
            content,
            ContentPart::File {
                filename: temp_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .expect("filename")
                    .to_string(),
                file_data: format!(
                    "data:application/pdf;base64,{}",
                    super::BASE64_STANDARD.encode(b"%PDF-1.4\n%mock\n")
                ),
            }
        );
    }

    #[test]
    fn load_supported_file_content_reads_docx_bytes() {
        let temp_path =
            std::env::temp_dir().join(format!("cai2036-runtime-{}.docx", std::process::id()));
        std::fs::write(&temp_path, b"PK\x03\x04mock-docx").expect("docx fixture should write");

        let content = super::load_supported_file_content(temp_path.to_str().expect("utf8 path"))
            .expect("docx content should load");

        let _ = std::fs::remove_file(&temp_path);

        assert_eq!(
            content,
            ContentPart::File {
                filename: temp_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .expect("filename")
                    .to_string(),
                file_data: format!(
                    "data:application/vnd.openxmlformats-officedocument.wordprocessingml.document;base64,{}",
                    super::BASE64_STANDARD.encode(b"PK\x03\x04mock-docx")
                ),
            }
        );
    }

    #[test]
    fn load_supported_file_content_reads_csv_bytes() {
        let temp_path =
            std::env::temp_dir().join(format!("cai2036-runtime-{}.csv", std::process::id()));
        std::fs::write(&temp_path, b"value\n2\n").expect("csv fixture should write");

        let content = super::load_supported_file_content(temp_path.to_str().expect("utf8 path"))
            .expect("csv content should load");

        let _ = std::fs::remove_file(&temp_path);

        assert_eq!(
            content,
            ContentPart::File {
                filename: temp_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .expect("filename")
                    .to_string(),
                file_data: format!(
                    "data:text/csv;base64,{}",
                    super::BASE64_STANDARD.encode(b"value\n2\n")
                ),
            }
        );
    }

    #[test]
    fn supported_file_media_type_maps_phase_three_extensions() {
        let cases = [
            ("report.pdf", "application/pdf"),
            ("report.doc", "application/msword"),
            ("report.dot", "application/msword"),
            (
                "report.docx",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            ),
            ("report.odt", "application/vnd.oasis.opendocument.text"),
            ("report.rtf", "application/rtf"),
            ("report.csv", "text/csv"),
            ("report.tsv", "text/tsv"),
            ("report.iif", "text/x-iif"),
            ("report.xla", "application/vnd.ms-excel"),
            ("report.xlb", "application/vnd.ms-excel"),
            ("report.xlc", "application/vnd.ms-excel"),
            ("report.xlm", "application/vnd.ms-excel"),
            ("report.xls", "application/vnd.ms-excel"),
            (
                "report.xlsx",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            ),
            ("report.xlt", "application/vnd.ms-excel"),
            ("report.xlw", "application/vnd.ms-excel"),
            ("report.pot", "application/vnd.ms-powerpoint"),
            ("report.ppa", "application/vnd.ms-powerpoint"),
            ("report.pps", "application/vnd.ms-powerpoint"),
            ("report.ppt", "application/vnd.ms-powerpoint"),
            (
                "report.pptx",
                "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            ),
            ("report.pwz", "application/vnd.ms-powerpoint"),
            ("report.wiz", "application/vnd.ms-powerpoint"),
        ];

        for (path, expected) in cases {
            let media_type = super::supported_file_media_type(Path::new(path))
                .unwrap_or_else(|err| panic!("expected media type for {path}: {err}"));
            assert_eq!(media_type, expected, "wrong media type for {path}");
        }
    }

    #[test]
    fn rejects_unsupported_extension_for_file_inputs() {
        let err = super::load_supported_file_content("./report.txt").expect_err("txt should fail");
        assert!(err.contains("Supported: pdf, docx, csv"));
        assert!(err.contains("pptx"));
    }
}

/// Allowlisted provider metadata retained even when output decoding fails later.
#[derive(Clone, Debug, Default)]
pub(crate) struct ProviderFacts {
    pub(crate) usage: Option<ProviderUsage>,
    pub(crate) resolved_model: Option<String>,
    pub(crate) provider_request_id: Option<String>,
    finish_reason: Option<String>,
}
impl ProviderFacts {
    pub(crate) fn from_body(body: &[u8], provider: super::ProviderKind) -> Self {
        serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .map(|value| Self::from_value(&value, provider))
            .unwrap_or_default()
    }
    pub(crate) fn from_value(value: &serde_json::Value, provider: super::ProviderKind) -> Self {
        use super::ProviderKind;
        let value = value.get("response").unwrap_or(value);
        let usage = value.get("usage");
        let get = |names: &[&str]| {
            names.iter().find_map(|name| {
                usage
                    .and_then(|usage| usage.get(*name))
                    .and_then(serde_json::Value::as_u64)
            })
        };
        let input = get(&["input_tokens", "prompt_tokens", "total_input_tokens"]).or_else(|| {
            value
                .get("prompt_eval_count")
                .and_then(serde_json::Value::as_u64)
        });
        let output = get(&["output_tokens", "completion_tokens", "total_output_tokens"])
            .or_else(|| value.get("eval_count").and_then(serde_json::Value::as_u64));
        let total = get(&["total_tokens"]).or_else(|| {
            if matches!(
                provider,
                ProviderKind::Anthropic | ProviderKind::TypeSafe | ProviderKind::Ollama
            ) {
                input.zip(output).and_then(|(a, b)| a.checked_add(b))
            } else {
                None
            }
        });
        let mut normalized = ProviderUsage {
            total_tokens_source: total.map(|_| {
                if get(&["total_tokens"]).is_some() {
                    "reported"
                } else {
                    "derived_input_plus_output"
                }
                .to_string()
            }),
            input_tokens: input,
            output_tokens: output,
            total_tokens: total,
            ..Default::default()
        };
        if let Some(usage) = usage {
            let mut input_details = serde_json::Map::new();
            let mut output_details = serde_json::Map::new();
            for key in [
                "cache_creation_input_tokens",
                "cache_read_input_tokens",
                "total_cached_tokens",
            ] {
                if let Some(count) = usage[key].as_u64() {
                    input_details.insert(key.into(), count.into());
                }
            }
            for key in ["total_thought_tokens", "total_tool_use_tokens"] {
                if let Some(count) = usage[key].as_u64() {
                    output_details.insert(key.into(), count.into());
                }
            }
            for key in ["input_tokens_details", "prompt_tokens_details"] {
                if let Some(details) = sanitize_token_details(&usage[key]) {
                    input_details.extend(details.as_object().unwrap().clone());
                }
            }
            for key in ["output_tokens_details", "completion_tokens_details"] {
                if let Some(details) = sanitize_token_details(&usage[key]) {
                    output_details.extend(details.as_object().unwrap().clone());
                }
            }
            if let Some(details) = sanitize_token_details(usage) {
                for (key, value) in details.as_object().unwrap() {
                    match key.as_str() {
                        "input_tokens_by_modality" => {
                            input_details.insert(key.clone(), value.clone());
                        }
                        "output_tokens_by_modality" => {
                            output_details.insert(key.clone(), value.clone());
                        }
                        _ => {}
                    }
                }
            }
            normalized.input_token_details =
                (!input_details.is_empty()).then_some(input_details.into());
            normalized.output_token_details =
                (!output_details.is_empty()).then_some(output_details.into());
        }
        Self {
            usage: (usage.is_some() || input.is_some() || output.is_some()).then_some(normalized),
            resolved_model: safe_metadata_identifier(value.get("model")),
            provider_request_id: safe_metadata_identifier(
                value.get("id").or_else(|| value.get("request_id")),
            ),
            finish_reason: safe_metadata_identifier(
                value
                    .get("stop_reason")
                    .or_else(|| value.pointer("/choices/0/finish_reason"))
                    .or_else(|| value.get("status")),
            ),
        }
    }
    pub(crate) fn merge_value(&mut self, value: &serde_json::Value, token: &str) {
        let facts = Self::from_value(value, super::ProviderKind::OpenAi).redact_token(token);
        if facts.usage.is_some() {
            self.usage = facts.usage;
        }
        if facts.resolved_model.is_some() {
            self.resolved_model = facts.resolved_model;
        }
        if facts.provider_request_id.is_some() {
            self.provider_request_id = facts.provider_request_id;
        }
        if facts.finish_reason.is_some() {
            self.finish_reason = facts.finish_reason;
        }
    }
    pub(crate) fn with_request_id(mut self, value: Option<&str>) -> Self {
        if let Some(value) = value {
            if let Some(safe) = safe_metadata_identifier(Some(&serde_json::json!(value))) {
                self.provider_request_id = Some(safe);
            }
        }
        self
    }
    pub(crate) fn redact_token(mut self, token: &str) -> Self {
        for field in [
            &mut self.resolved_model,
            &mut self.provider_request_id,
            &mut self.finish_reason,
        ] {
            if !token.is_empty() && field.as_deref().is_some_and(|value| value.contains(token)) {
                *field = None;
            }
        }
        self
    }
    pub(crate) fn error(&self, mut error: super::ProviderError) -> super::ProviderError {
        error.usage = self.usage.clone();
        error.resolved_model = self.resolved_model.clone();
        error.provider_request_id = self.provider_request_id.clone();
        error.finish_reason = self.finish_reason.clone();
        error
    }
    pub(crate) fn text(&self, mut response: ProviderTextResponse) -> ProviderTextResponse {
        response.resolved_model = self.resolved_model.clone().or(response.resolved_model);
        response.provider_request_id = self.provider_request_id.clone();
        response.finish_reason = self.finish_reason.clone();
        response
    }
    pub(crate) fn image(&self, mut response: ProviderImageResponse) -> ProviderImageResponse {
        response.resolved_model = self.resolved_model.clone();
        response.provider_request_id = self.provider_request_id.clone();
        response.finish_reason = self.finish_reason.clone();
        response
    }
}

fn safe_metadata_identifier(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-/:".contains(&byte))
        })
        .map(str::to_owned)
}

pub(crate) fn sanitize_token_details(value: &serde_json::Value) -> Option<serde_json::Value> {
    let mut result = serde_json::Map::new();
    for key in [
        "cached_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
        "total_cached_tokens",
        "reasoning_tokens",
        "audio_tokens",
        "text_tokens",
        "image_tokens",
        "accepted_prediction_tokens",
        "rejected_prediction_tokens",
        "total_thought_tokens",
        "total_tool_use_tokens",
    ] {
        if let Some(count) = value[key].as_u64() {
            result.insert(key.into(), count.into());
        }
    }
    // Modality records have a closed label/count contract; arbitrary nested values stay out.
    for key in ["input_tokens_by_modality", "output_tokens_by_modality"] {
        if let Some(items) = value[key].as_array() {
            let items: Vec<_> = items
                .iter()
                .take(16)
                .filter_map(|item| {
                    let modality = item["modality"].as_str()?;
                    if !matches!(
                        modality,
                        "text"
                            | "image"
                            | "audio"
                            | "video"
                            | "document"
                            | "TEXT"
                            | "IMAGE"
                            | "AUDIO"
                            | "VIDEO"
                            | "DOCUMENT"
                    ) {
                        return None;
                    }
                    let count = item["token_count"]
                        .as_u64()
                        .or_else(|| item["tokenCount"].as_u64())
                        .or_else(|| item["tokens"].as_u64())?;
                    Some(serde_json::json!({"modality":modality,"token_count":count}))
                })
                .collect();
            if !items.is_empty() {
                result.insert(key.into(), items.into());
            }
        }
    }
    (!result.is_empty()).then_some(result.into())
}

#[cfg(test)]
mod usage_fact_tests {
    use super::super::{ProviderError, ProviderKind};
    use super::*;
    use serde_json::json;
    #[test]
    fn provider_completion_survives_later_validation_failure_for_every_transport() {
        for (provider, payload) in [
            (
                ProviderKind::OpenAi,
                json!({"id":"req_1","model":"returned-model","usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}),
            ),
            (
                ProviderKind::Mistral,
                json!({"id":"req_1","model":"returned-model","usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}),
            ),
            (
                ProviderKind::Ollama,
                json!({"model":"returned-model","prompt_eval_count":3,"eval_count":2}),
            ),
            (
                ProviderKind::Anthropic,
                json!({"id":"req_1","model":"returned-model","stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2}}),
            ),
            (
                ProviderKind::Gemini,
                json!({"id":"req_1","model":"returned-model","usage":{"total_input_tokens":3,"total_output_tokens":2,"total_tokens":5}}),
            ),
            (
                ProviderKind::Xai,
                json!({"id":"req_1","model":"returned-model","status":"completed","usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}}),
            ),
            (
                ProviderKind::TypeSafe,
                json!({"model":"returned-model","usage":{"input_tokens":3,"output_tokens":2}}),
            ),
        ] {
            let facts = ProviderFacts::from_value(&payload, provider);
            let error = facts.error(ProviderError::invalid_response(provider, "Invalid output"));
            assert_eq!(error.usage.as_ref().unwrap().input_tokens, Some(3));
            assert_eq!(error.usage.as_ref().unwrap().total_tokens, Some(5));
            assert_eq!(error.resolved_model.as_deref(), Some("returned-model"));
        }
    }
    #[test]
    fn details_are_allowlisted_and_missing_counts_do_not_become_zero() {
        let facts=ProviderFacts::from_value(&json!({"model":"secret\noutput","id":"secret-token","usage":{"input_tokens":0,"output_tokens_details":{"reasoning_tokens":2,"prompt":"never persist","payload":{"secret":"never"}}}}),ProviderKind::OpenAi).redact_token("secret-token");
        assert!(facts.provider_request_id.is_none());
        assert!(facts.resolved_model.is_none());
        let usage = facts.usage.unwrap();
        assert_eq!(usage.input_tokens, Some(0));
        assert_eq!(usage.output_tokens, None);
        assert_eq!(usage.total_tokens, None);
        assert_eq!(
            usage.output_token_details,
            Some(json!({"reasoning_tokens":2}))
        );
    }
    #[test]
    fn native_total_overflow_is_unknown_and_prior_stream_facts_survive() {
        let facts = ProviderFacts::from_value(
            &json!({"prompt_eval_count":u64::MAX,"eval_count":1}),
            ProviderKind::Ollama,
        );
        assert_eq!(facts.usage.unwrap().total_tokens, None);
        let mut facts = ProviderFacts::default();
        facts.merge_value(&json!({"response":{"model":"returned","usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}}}),"");
        facts.merge_value(&json!({"type":"response.failed"}), "");
        assert_eq!(facts.usage.unwrap().total_tokens, Some(3));
    }
}
