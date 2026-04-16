//! Cargo AI protocol adapter for this tool.
//!
//! Tool-specific behavior lives in `tool.rs`. Keep this file focused on the
//! stable `describe` / `invoke` command protocol unless you are changing the
//! tool runtime contract itself.

mod agent_bridge;
mod tool;

#[allow(unused_imports)]
pub(crate) use agent_bridge::{
    ActionExecutionMode, AgentInputMode, ChildAgentOutcome, ChildAgentRequest, InvocationContext,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::io;

const PROTOCOL_VERSION: u32 = 1;

pub fn run(command: Option<&str>) -> Result<(), ToolError> {
    match command {
        Some("describe") => {
            let stdout = io::stdout();
            serde_json::to_writer_pretty(stdout.lock(), &describe_document())?;
            println!();
            Ok(())
        }
        Some("invoke") => {
            let stdin = io::stdin();
            let request: InvokeRequest = serde_json::from_reader(stdin.lock())?;
            let response = invoke(request)?;
            let stdout = io::stdout();
            serde_json::to_writer(stdout.lock(), &response)?;
            println!();
            Ok(())
        }
        Some(other) => Err(ToolError::new(format!(
            "Unsupported command '{other}'. Use 'describe' or 'invoke'."
        ))),
        None => Err(ToolError::new(
            "Missing command. Use 'describe' or 'invoke'.",
        )),
    }
}

fn describe_document() -> DescribeDocument {
    DescribeDocument {
        protocol_version: PROTOCOL_VERSION,
        name: tool::TOOL_NAME.to_string(),
        description: tool::description().to_string(),
        params: tool::params(),
        result: tool::result(),
        resource_profile: tool::resource_profile(),
        self_test: tool::self_test(),
        examples: Examples {
            minimal_invoke: InvokeRequest {
                protocol_version: PROTOCOL_VERSION,
                params: tool::minimal_example_params(),
                runtime_context: None,
            },
            full_invoke: InvokeRequest {
                protocol_version: PROTOCOL_VERSION,
                params: tool::full_example_params(),
                runtime_context: None,
            },
        },
    }
}

fn invoke(request: InvokeRequest) -> Result<InvokeResponse, ToolError> {
    if request.protocol_version != PROTOCOL_VERSION {
        return Err(ToolError::new(format!(
            "Unsupported protocol_version {}. Expected {}.",
            request.protocol_version, PROTOCOL_VERSION
        )));
    }

    Ok(InvokeResponse {
        protocol_version: PROTOCOL_VERSION,
        result: tool::invoke(request.params, request.runtime_context.unwrap_or_default())?,
    })
}

#[derive(Debug)]
pub struct ToolError(String);

impl ToolError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<io::Error> for ToolError {
    fn from(error: io::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<serde_json::Error> for ToolError {
    fn from(error: serde_json::Error) -> Self {
        Self(error.to_string())
    }
}

#[derive(Debug, Serialize)]
struct DescribeDocument {
    protocol_version: u32,
    name: String,
    description: String,
    params: BTreeMap<String, ParamSpec>,
    result: ResultSpec,
    resource_profile: ResourceProfile,
    self_test: SelfTestSpec,
    examples: Examples,
}

#[derive(Debug, Serialize)]
pub(crate) struct ParamSpec {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) required: bool,
    pub(crate) description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) default: Option<Value>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ResultSpec {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) nullable: bool,
    pub(crate) description: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ResourceProfile {
    pub(crate) network: AccessLevel,
    pub(crate) filesystem_read: AccessLevel,
    pub(crate) filesystem_write: AccessLevel,
    pub(crate) subprocess: AccessLevel,
    pub(crate) env_read: AccessLevel,
    pub(crate) credential_access: AccessLevel,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub(crate) enum AccessLevel {
    None,
    Optional,
    Required,
}

#[derive(Debug, Serialize)]
pub(crate) struct SelfTestSpec {
    pub(crate) supported: bool,
    pub(crate) safe: bool,
    pub(crate) description: String,
}

#[derive(Debug, Serialize)]
struct Examples {
    minimal_invoke: InvokeRequest,
    full_invoke: InvokeRequest,
}

#[derive(Debug, Deserialize, Serialize)]
struct InvokeRequest {
    protocol_version: u32,
    #[serde(default)]
    params: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime_context: Option<InvocationContext>,
}

#[derive(Debug, Serialize)]
struct InvokeResponse {
    protocol_version: u32,
    result: Option<String>,
}
