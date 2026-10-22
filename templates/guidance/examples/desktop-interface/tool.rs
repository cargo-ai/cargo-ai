//! Deterministic preview only: no filesystem, network, environment or child work.
use crate::{
    AccessLevel, InvocationContext, ParamSpec, ResourceProfile, ResultSpec, SelfTestSpec, ToolError,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) const TOOL_NAME: &str = "desktop_preview";
pub(crate) fn description() -> &'static str {
    "Return a bounded note as structured preview data without saving it."
}
pub(crate) fn params() -> BTreeMap<String, ParamSpec> {
    BTreeMap::from([(
        "text".into(),
        ParamSpec {
            kind: "string".into(),
            required: true,
            description: "A note of 1–4096 UTF-8 bytes.".into(),
            default: None,
        },
    )])
}
pub(crate) fn result() -> ResultSpec {
    ResultSpec {
        kind: "string".into(),
        nullable: true,
        description: "JSON selected-result wrapper containing status and text.".into(),
    }
}
pub(crate) fn resource_profile() -> ResourceProfile {
    ResourceProfile {
        network: AccessLevel::None,
        filesystem_read: AccessLevel::None,
        filesystem_write: AccessLevel::None,
        subprocess: AccessLevel::None,
        env_read: AccessLevel::None,
        credential_access: AccessLevel::None,
    }
}
pub(crate) fn self_test() -> SelfTestSpec {
    SelfTestSpec {
        supported: false,
        safe: false,
        description: "Run the declared local preview action to verify its selected result.".into(),
    }
}
pub(crate) fn minimal_example_params() -> BTreeMap<String, Value> {
    BTreeMap::from([("text".into(), json!("Hello Desktop"))])
}
pub(crate) fn full_example_params() -> BTreeMap<String, Value> {
    minimal_example_params()
}
pub(crate) fn invoke(
    params: BTreeMap<String, Value>,
    _context: InvocationContext,
) -> Result<Option<String>, ToolError> {
    let text = params
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("text must be a string"))?;
    if params.len() != 1 || text.is_empty() || text.len() > 4096 {
        return Err(ToolError::new(
            "Provide only text, containing 1–4096 UTF-8 bytes",
        ));
    }
    Ok(Some(
        json!({"data":{"status":"previewed","text":text},"artifacts":[]}).to_string(),
    ))
}
