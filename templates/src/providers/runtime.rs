use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::{Deserialize, Serialize}; // Data format (e.g.,JSON, TOML) (de)serialization
use serde_json;
use std::{collections::BTreeMap, fs, path::Path};

const SUPPORTED_FILE_EXTENSIONS_MESSAGE: &str =
    "pdf, docx, csv, xla, xlb, xlc, xlm, xls, xlsx, xlt, xlw, tsv, iif, doc, dot, odt, rtf, pot, ppa, pps, ppt, pptx, pwz, wiz";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ContentPart {
    Text(String),
    Image { data_url: String },
    File { filename: String, file_data: String },
}

pub(crate) trait ValidatedResponse {
    fn validate_response(&self) -> Result<(), String>;
}

#[derive(Debug)]
pub struct Cargo<T: for<'de> Deserialize<'de> + Serialize + Clone + ValidatedResponse> {
    inputs: Vec<ContentPart>,
    context: String,
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

    pub fn set_response(&mut self, response: String) -> bool {
        match serde_json::from_str::<T>(&response) {
            Ok(response) => match response.validate_response() {
                Ok(()) => {
                    self.response = Some(response);
                    true
                }
                Err(_) => {
                    self.response = None;
                    false
                }
            },
            Err(_) => {
                // Keep state deterministic: failed parse must not retain stale success output.
                self.response = None;
                false
            }
        }
    }

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
    let image_bytes = fs::read(image_path)
        .map_err(|error| format!("Failed to read image input '{}': {error}", image_path.display()))?;
    let media_type = image_media_type(image_path)?;
    let encoded = BASE64_STANDARD.encode(image_bytes);
    Ok(format!("data:{media_type};base64,{encoded}"))
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
    use serde::{Deserialize, Serialize};

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
    fn detects_when_content_parts_include_images() {
        let text_only = [ContentPart::Text("x".to_string())];
        assert!(
            !text_only
                .iter()
                .any(|part| matches!(part, ContentPart::Image { .. }))
        );

        let with_image = [
            ContentPart::Text("x".to_string()),
            ContentPart::Image {
                data_url: "data:image/png;base64,abc".to_string(),
            },
        ];
        assert!(
            with_image
                .iter()
                .any(|part| matches!(part, ContentPart::Image { .. }))
        );
    }
}
