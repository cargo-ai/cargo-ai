//! Focused NDJSON usage ledger for Cargo AI-owned runtime work.

use crate::providers::{ProviderKind, ProviderUsage};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

pub(crate) const USAGE_LOG_ENV: &str = "CARGO_AI_USAGE_LOG";
pub(crate) const USAGE_ROOT_RUN_ID_ENV: &str = "CARGO_AI_USAGE_ROOT_RUN_ID";
pub(crate) const USAGE_PARENT_AGENT_RUN_ID_ENV: &str = "CARGO_AI_USAGE_PARENT_AGENT_RUN_ID";
pub(crate) const USAGE_LAUNCHED_BY_TYPE_ENV: &str = "CARGO_AI_USAGE_LAUNCHED_BY_TYPE";
pub(crate) const USAGE_LAUNCHED_BY_ACTION_ENV: &str = "CARGO_AI_USAGE_LAUNCHED_BY_ACTION";
pub(crate) const USAGE_LAUNCHED_BY_TOOL_ENV: &str = "CARGO_AI_USAGE_LAUNCHED_BY_TOOL";
pub(crate) const USAGE_LAUNCHED_BY_STEP_INDEX_ENV: &str = "CARGO_AI_USAGE_LAUNCHED_BY_STEP_INDEX";

const USAGE_LOG_FORMAT_VERSION: &str = "2026-06-10.r1";

#[derive(Clone, Debug)]
pub(crate) struct UsageLogContext {
    sink: Arc<UsageLogSink>,
    root_run_id: String,
    agent_run_id: String,
    parent_agent_run_id: Option<String>,
    depth: u32,
    root_owner: bool,
    agent: Option<Value>,
}

#[derive(Debug)]
struct UsageLogSink {
    path: PathBuf,
    write_lock: Mutex<()>,
}

#[derive(Debug, Clone)]
pub(crate) struct UsageLaunchedBy {
    pub(crate) kind: &'static str,
    pub(crate) action: Option<String>,
    pub(crate) tool: Option<String>,
    pub(crate) step_index: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct UsageStep {
    pub(crate) kind: &'static str,
    pub(crate) action: Option<String>,
    pub(crate) step_index: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct UsageTool {
    pub(crate) name: String,
    pub(crate) action: String,
    pub(crate) step_index: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct UsageProviderRequest<'a> {
    pub(crate) provider: ProviderKind,
    pub(crate) profile_name: Option<&'a str>,
    pub(crate) auth_mode: &'a str,
    pub(crate) model: &'a str,
    pub(crate) step: UsageStep,
    pub(crate) usage: Option<&'a ProviderUsage>,
    pub(crate) duration: Duration,
    pub(crate) status: UsageStatus,
    pub(crate) error: Option<UsageError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsageStatus {
    Success,
    Failed,
}

#[derive(Debug, Clone)]
pub(crate) struct UsageError {
    pub(crate) kind: String,
    pub(crate) http_status: Option<u16>,
    pub(crate) message: String,
}

pub(crate) struct UsageAgentRunGuard {
    context: UsageLogContext,
    started_at: Instant,
    completed: bool,
}

pub(crate) struct UsageToolRunGuard {
    context: UsageLogContext,
    tool: UsageTool,
    started_at: Instant,
    completed: bool,
}

impl UsageLogContext {
    pub(crate) fn from_runtime(
        explicit_path: Option<&str>,
        depth: u32,
        agent: Option<Value>,
    ) -> Result<Option<(Self, UsageAgentRunGuard)>, String> {
        let path = explicit_path
            .and_then(non_empty_string)
            .or_else(|| std::env::var(USAGE_LOG_ENV).ok().and_then(|v| non_empty_string(&v)));
        let Some(path) = path else {
            return Ok(None);
        };

        let inherited_root_run_id =
            std::env::var(USAGE_ROOT_RUN_ID_ENV).ok().and_then(|v| non_empty_string(&v));
        let root_owner = inherited_root_run_id.is_none();
        let root_run_id =
            inherited_root_run_id.unwrap_or_else(|| format!("cai_run_{}", Uuid::now_v7()));
        let agent_run_id = format!("cai_agent_run_{}", Uuid::now_v7());
        let parent_agent_run_id = std::env::var(USAGE_PARENT_AGENT_RUN_ID_ENV)
            .ok()
            .and_then(|v| non_empty_string(&v));
        let launched_by = launched_by_from_env();

        let context = Self {
            sink: Arc::new(UsageLogSink {
                path: PathBuf::from(path),
                write_lock: Mutex::new(()),
            }),
            root_run_id,
            agent_run_id,
            parent_agent_run_id,
            depth,
            root_owner,
            agent,
        };

        if context.root_owner {
            context.write_event(json!({
                "event_type": "usage_log_started",
                "usage_log_format_version": USAGE_LOG_FORMAT_VERSION,
                "timestamp": timestamp(),
                "root_run_id": context.root_run_id,
            }))?;
        }

        let mut event = context.agent_event_base("agent_run_started");
        if let Some(launched_by) = launched_by {
            event["launched_by"] = launched_by.to_json();
        }
        context.write_event(event)?;

        let guard = UsageAgentRunGuard {
            context: context.clone(),
            started_at: Instant::now(),
            completed: false,
        };

        Ok(Some((context, guard)))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.sink.path
    }

    pub(crate) fn direct_child_env(
        &self,
        launched_by: UsageLaunchedBy,
    ) -> Vec<(&'static str, String)> {
        self.child_env(launched_by)
    }

    pub(crate) fn tool_bridge_context(
        &self,
        tool_name: &str,
        action_name: &str,
        step_index: Option<usize>,
    ) -> Value {
        json!({
            "path": self.path().display().to_string(),
            "root_run_id": self.root_run_id,
            "parent_agent_run_id": self.agent_run_id,
            "launched_by": UsageLaunchedBy {
                kind: "tool_bridge",
                action: Some(action_name.to_string()),
                tool: Some(tool_name.to_string()),
                step_index,
            }.to_json()
        })
    }

    pub(crate) fn record_provider_request(&self, request: UsageProviderRequest<'_>) {
        let mut event = self.agent_event_base("provider_request_completed");
        event["step"] = request.step.to_json();
        event["provider"] = provider_json(
            request.provider,
            request.profile_name,
            request.auth_mode,
            request.model,
        );
        event["duration_ms"] = json!(duration_ms(request.duration));
        event["timing"] = json!({
            "provider_round_trip_ms": duration_ms(request.duration),
        });
        event["status"] = json!(request.status.as_str());
        if let Some(usage) = request.usage {
            event["usage"] = usage_json(usage);
        } else {
            event["usage"] = Value::Null;
        }
        if let Some(error) = request.error {
            event["error"] = error.to_json();
        }
        self.write_event_lossy(event);
    }

    pub(crate) fn start_tool_run(&self, tool: UsageTool) -> UsageToolRunGuard {
        let mut event = self.agent_event_base("tool_run_started");
        event["tool"] = tool.to_json();
        self.write_event_lossy(event);
        UsageToolRunGuard {
            context: self.clone(),
            tool,
            started_at: Instant::now(),
            completed: false,
        }
    }

    fn record_tool_completed(&self, tool: &UsageTool, duration: Duration, status: UsageStatus) {
        let mut event = self.agent_event_base("tool_run_completed");
        event["tool"] = tool.to_json();
        event["duration_ms"] = json!(duration_ms(duration));
        event["status"] = json!(status.as_str());
        self.write_event_lossy(event);
    }

    fn record_agent_completed(&self, duration: Duration, status: UsageStatus) {
        let mut event = self.agent_event_base("agent_run_completed");
        event["duration_ms"] = json!(duration_ms(duration));
        event["status"] = json!(status.as_str());
        self.write_event_lossy(event);

        if self.root_owner {
            self.write_event_lossy(json!({
                "event_type": "root_run_completed",
                "timestamp": timestamp(),
                "root_run_id": self.root_run_id,
                "duration_ms": duration_ms(duration),
                "status": status.as_str(),
            }));
        }
    }

    fn child_env(&self, launched_by: UsageLaunchedBy) -> Vec<(&'static str, String)> {
        let mut env = vec![
            (USAGE_LOG_ENV, self.path().display().to_string()),
            (USAGE_ROOT_RUN_ID_ENV, self.root_run_id.clone()),
            (USAGE_PARENT_AGENT_RUN_ID_ENV, self.agent_run_id.clone()),
            (USAGE_LAUNCHED_BY_TYPE_ENV, launched_by.kind.to_string()),
        ];
        if let Some(action) = launched_by.action {
            env.push((USAGE_LAUNCHED_BY_ACTION_ENV, action));
        }
        if let Some(tool) = launched_by.tool {
            env.push((USAGE_LAUNCHED_BY_TOOL_ENV, tool));
        }
        if let Some(step_index) = launched_by.step_index {
            env.push((USAGE_LAUNCHED_BY_STEP_INDEX_ENV, step_index.to_string()));
        }
        env
    }

    fn agent_event_base(&self, event_type: &str) -> Value {
        let mut event = json!({
            "event_type": event_type,
            "timestamp": timestamp(),
            "root_run_id": self.root_run_id,
            "agent_run_id": self.agent_run_id,
            "parent_agent_run_id": self.parent_agent_run_id,
            "depth": self.depth,
        });
        if let Some(agent) = self.agent.as_ref() {
            event["agent"] = agent.clone();
        }
        event
    }

    fn write_event(&self, event: Value) -> Result<(), String> {
        let _guard = self
            .sink
            .write_lock
            .lock()
            .map_err(|_| "usage log writer lock was poisoned".to_string())?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.sink.path)
            .map_err(|error| {
                format!(
                    "failed to open usage log '{}': {error}",
                    self.sink.path.display()
                )
            })?;
        let mut line = serde_json::to_vec(&event)
            .map_err(|error| format!("failed to serialize usage log event: {error}"))?;
        line.push(b'\n');
        file.write_all(&line).map_err(|error| {
            format!(
                "failed to write usage log '{}': {error}",
                self.sink.path.display()
            )
        })
    }

    fn write_event_lossy(&self, event: Value) {
        if let Err(error) = self.write_event(event) {
            eprintln!("x Failed to write usage log event: {error}");
        }
    }
}

impl UsageAgentRunGuard {
    pub(crate) fn finish_success(&mut self) {
        self.finish(UsageStatus::Success);
    }

    pub(crate) fn finish_failed(&mut self) {
        self.finish(UsageStatus::Failed);
    }

    fn finish(&mut self, status: UsageStatus) {
        if self.completed {
            return;
        }
        self.completed = true;
        self.context
            .record_agent_completed(self.started_at.elapsed(), status);
    }
}

impl Drop for UsageAgentRunGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.finish(UsageStatus::Failed);
        }
    }
}

impl UsageToolRunGuard {
    pub(crate) fn finish_success(&mut self) {
        self.finish(UsageStatus::Success);
    }

    fn finish(&mut self, status: UsageStatus) {
        if self.completed {
            return;
        }
        self.completed = true;
        self.context
            .record_tool_completed(&self.tool, self.started_at.elapsed(), status);
    }
}

impl Drop for UsageToolRunGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.finish(UsageStatus::Failed);
        }
    }
}

impl UsageLaunchedBy {
    fn to_json(&self) -> Value {
        let mut value = json!({ "type": self.kind });
        if let Some(action) = self.action.as_deref() {
            value["action"] = json!(action);
        }
        if let Some(tool) = self.tool.as_deref() {
            value["tool"] = json!(tool);
        }
        if let Some(step_index) = self.step_index {
            value["step_index"] = json!(step_index);
        }
        value
    }
}

impl UsageStep {
    fn to_json(&self) -> Value {
        let mut value = json!({ "kind": self.kind });
        if let Some(action) = self.action.as_deref() {
            value["action"] = json!(action);
        }
        if let Some(step_index) = self.step_index {
            value["step_index"] = json!(step_index);
        }
        value
    }
}

impl UsageTool {
    fn to_json(&self) -> Value {
        let mut value = json!({
            "name": self.name,
            "action": self.action,
        });
        if let Some(step_index) = self.step_index {
            value["step_index"] = json!(step_index);
        }
        value
    }
}

impl UsageStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failed => "failed",
        }
    }
}

impl UsageError {
    pub(crate) fn redacted(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            http_status: None,
            message: message.into(),
        }
    }

    fn to_json(&self) -> Value {
        let mut value = json!({
            "kind": self.kind,
            "message": self.message,
        });
        if let Some(status) = self.http_status {
            value["http_status"] = json!(status);
        }
        value
    }
}

fn provider_json(
    provider: ProviderKind,
    profile_name: Option<&str>,
    auth_mode: &str,
    model: &str,
) -> Value {
    json!({
        "server": match provider {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Gemini => "gemini",
            ProviderKind::Ollama => "ollama",
            ProviderKind::OpenAi => "openai",
        },
        "profile": profile_name,
        "model": model,
        "auth_mode": auth_mode,
    })
}

fn usage_json(usage: &ProviderUsage) -> Value {
    to_json_without_nulls(usage)
}

fn to_json_without_nulls<T: Serialize>(value: &T) -> Value {
    match serde_json::to_value(value).unwrap_or(Value::Null) {
        Value::Object(mut map) => {
            map.retain(|_, value| !value.is_null());
            Value::Object(map)
        }
        other => other,
    }
}

fn launched_by_from_env() -> Option<UsageLaunchedBy> {
    let kind = std::env::var(USAGE_LAUNCHED_BY_TYPE_ENV)
        .ok()
        .and_then(|v| non_empty_string(&v))?;
    let kind = match kind.as_str() {
        "agent_step" => "agent_step",
        "tool_bridge" => "tool_bridge",
        _ => "unknown",
    };
    Some(UsageLaunchedBy {
        kind,
        action: std::env::var(USAGE_LAUNCHED_BY_ACTION_ENV)
            .ok()
            .and_then(|v| non_empty_string(&v)),
        tool: std::env::var(USAGE_LAUNCHED_BY_TOOL_ENV)
            .ok()
            .and_then(|v| non_empty_string(&v)),
        step_index: std::env::var(USAGE_LAUNCHED_BY_STEP_INDEX_ENV)
            .ok()
            .and_then(|v| v.parse::<usize>().ok()),
    })
}

fn timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn non_empty_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderUsage;

    #[test]
    fn usage_log_writes_one_json_object_per_line() {
        let path = std::env::temp_dir().join(format!("cargo-ai-usage-{}.ndjson", Uuid::now_v7()));
        let (context, mut guard) =
            UsageLogContext::from_runtime(Some(path.to_str().unwrap()), 0, None)
                .expect("usage log should initialize")
                .expect("usage log should be enabled");

        context.record_provider_request(UsageProviderRequest {
            provider: ProviderKind::OpenAi,
            profile_name: Some("openai-account"),
            auth_mode: "openai_account",
            model: "gpt-5",
            step: UsageStep {
                kind: "agent_inference",
                action: None,
                step_index: None,
            },
            usage: Some(&ProviderUsage {
                input_tokens: Some(10),
                output_tokens: Some(4),
                total_tokens: Some(14),
                input_token_details: None,
                output_token_details: None,
            }),
            duration: Duration::from_millis(42),
            status: UsageStatus::Success,
            error: None,
        });
        guard.finish_success();

        let contents = std::fs::read_to_string(&path).expect("usage log should be readable");
        let lines = contents.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 5);
        for line in &lines {
            serde_json::from_str::<Value>(line).expect("line should be json");
        }
        assert_eq!(
            serde_json::from_str::<Value>(lines[0]).unwrap()["event_type"],
            "usage_log_started"
        );
        assert_eq!(
            serde_json::from_str::<Value>(lines[2]).unwrap()["event_type"],
            "provider_request_completed"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn failed_provider_event_uses_redacted_error_metadata() {
        let path = std::env::temp_dir().join(format!("cargo-ai-usage-{}.jsonl", Uuid::now_v7()));
        let (context, mut guard) =
            UsageLogContext::from_runtime(Some(path.to_str().unwrap()), 0, None)
                .expect("usage log should initialize")
                .expect("usage log should be enabled");

        context.record_provider_request(UsageProviderRequest {
            provider: ProviderKind::Ollama,
            profile_name: None,
            auth_mode: "none",
            model: "llama3.2",
            step: UsageStep {
                kind: "agent_inference",
                action: None,
                step_index: None,
            },
            usage: None,
            duration: Duration::from_millis(12),
            status: UsageStatus::Failed,
            error: Some(UsageError::redacted(
                "invalid_response",
                "Provider request failed.",
            )),
        });
        guard.finish_failed();

        let contents = std::fs::read_to_string(&path).expect("usage log should be readable");
        assert!(!contents.contains("sk-secret"));
        assert!(!contents.contains("prompt"));
        assert!(!contents.contains("raw response body"));
        assert!(contents.contains("\"status\":\"failed\""));
        assert!(contents.contains("\"usage\":null"));
        let _ = std::fs::remove_file(path);
    }
}
