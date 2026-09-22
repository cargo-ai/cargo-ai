//! Automatic metadata history and optional legacy NDJSON export.

use crate::providers::{ProviderKind, ProviderUsage};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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
    tracking: bool,
    pending: Mutex<Vec<Value>>,
    warned: AtomicBool,
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
    pub(crate) attempt: &'a UsageProviderAttempt,
    pub(crate) provider: ProviderKind,
    pub(crate) profile_name: Option<&'a str>,
    pub(crate) auth_mode: &'a str,
    pub(crate) model: &'a str,
    pub(crate) resolved_model: Option<&'a str>,
    pub(crate) provider_request_id: Option<&'a str>,
    pub(crate) finish_reason: Option<&'a str>,
    pub(crate) step: UsageStep,
    pub(crate) usage: Option<&'a ProviderUsage>,
    pub(crate) duration: Duration,
    pub(crate) status: UsageStatus,
    pub(crate) error: Option<UsageError>,
}

#[derive(Debug, Clone)]
pub(crate) struct UsageProviderAttempt {
    operation_id: String,
    attempt_id: String,
    started_at: String,
    start_event: Value,
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
    operation_id: String,
    started_at: Instant,
    completed: bool,
}

impl UsageLogContext {
    pub(crate) fn from_runtime(
        explicit_path: Option<&str>,
        depth: u32,
        agent: Option<Value>,
    ) -> Result<Option<(Self, UsageAgentRunGuard)>, String> {
        let path = explicit_path.and_then(non_empty_string).or_else(|| {
            std::env::var(USAGE_LOG_ENV)
                .ok()
                .and_then(|v| non_empty_string(&v))
        });
        let tracking = match crate::usage_store::tracking_enabled() {
            Ok(enabled) => enabled,
            Err(error) => {
                eprintln!("Usage capture unavailable: {error}; history may be incomplete.");
                false
            }
        };
        if path.is_none() && !tracking {
            return Ok(None);
        }
        let path = path.unwrap_or_default();

        let inherited_root_run_id = std::env::var(USAGE_ROOT_RUN_ID_ENV)
            .ok()
            .and_then(|v| non_empty_string(&v));
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
                tracking,
                pending: Mutex::new(Vec::new()),
                warned: AtomicBool::new(false),
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
                "agent_run_id": context.agent_run_id,
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

    pub(crate) fn start_provider_request(
        &self,
        provider: ProviderKind,
        profile_name: Option<&str>,
        auth_mode: &str,
        model: &str,
        step: UsageStep,
    ) -> UsageProviderAttempt {
        let mut attempt = UsageProviderAttempt {
            operation_id: format!("cai_operation_{}", Uuid::now_v7()),
            attempt_id: format!("cai_attempt_{}", Uuid::now_v7()),
            started_at: timestamp(),
            start_event: Value::Null,
        };
        let mut event = self.agent_event_base("provider_request_started");
        event["timestamp"] = json!(attempt.started_at);
        event["started_at"] = json!(attempt.started_at);
        event["step"] = step.to_json();
        event["provider"] = provider_json(provider, profile_name, auth_mode, model);
        event["provider"]["requested_model"] = json!(model);
        event["provider"]["resolved_model"] = Value::Null;
        event["operation_id"] = json!(attempt.operation_id);
        event["attempt_id"] = json!(attempt.attempt_id);
        event["attempt_index"] = json!(1);
        event["retry_count"] = json!(0);
        event["attempt_coverage"] = json!("cargo_ai_observed_request");
        event["status"] = json!("started");
        event["provider_completion"] = json!("unknown");
        event["timestamp_basis"] = json!("observed_request_start");
        attempt.start_event = event.clone();
        // Starts belong to automatic history; legacy exports retain their completion-only order.
        let _ = self.write_event_to_sinks(event, false);
        attempt
    }

    pub(crate) fn abort_provider_before_dispatch(&self, attempt: &UsageProviderAttempt) {
        let mut event = attempt.start_event.clone();
        event["event_type"] = json!("provider_request_completed");
        event["timestamp"] = json!(timestamp());
        event["ended_at"] = event["timestamp"].clone();
        event["timestamp_basis"] = json!("observed_request_start_and_completion");
        event["status"] = json!("failed");
        event["provider_completion"] = json!("not_dispatched");
        event["usage"] = Value::Null;
        event["error"] = UsageError::redacted(
            "timeout",
            "Invocation deadline expired before provider dispatch.",
        )
        .to_json();
        self.write_event_lossy(event);
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
        event["provider"]["requested_model"] = json!(request.model);
        event["provider"]["resolved_model"] = json!(request.resolved_model);
        event["provider_request_id"] = json!(request.provider_request_id);
        event["finish_reason"] = json!(request.finish_reason);
        event["operation_id"] = json!(request.attempt.operation_id);
        event["attempt_id"] = json!(request.attempt.attempt_id);
        event["attempt_index"] = json!(1);
        event["retry_count"] = json!(0);
        event["attempt_coverage"] = json!("cargo_ai_observed_request");
        event["coverage"] = json!({
            "input_tokens": if request.usage.and_then(|u|u.input_tokens).is_some() { "reported" } else { "unavailable" },
            "output_tokens": if request.usage.and_then(|u|u.output_tokens).is_some() { "reported" } else { "unavailable" },
            "total_tokens": if request.usage.and_then(|u|u.total_tokens).is_some() {
                request.usage.and_then(|usage|usage.total_tokens_source.as_deref()).unwrap_or("reported")
            } else { "unavailable" },
            "normalization_version": 1,
            "detail_semantics": "provider_native_details_may_overlap_headline_counters"
        });
        event["duration_ms"] = json!(duration_ms(request.duration));
        add_time_range(&mut event, request.duration);
        event["started_at"] = json!(request.attempt.started_at);
        event["timestamp_basis"] = json!("observed_request_start_and_completion");
        event["timing"] = json!({
            "provider_round_trip_ms": duration_ms(request.duration),
        });
        event["status"] = json!(request.status.as_str());
        event["provider_completion"] =
            json!(if request.status == UsageStatus::Success
                || request.usage.is_some()
                || request.error.as_ref().is_some_and(
                    |error| error.http_status.is_some() || error.kind == "invalidresponse"
                ) {
                "response_received"
            } else {
                "unknown_or_failed_before_response"
            });
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
        let operation_id = format!("cai_tool_run_{}", Uuid::now_v7());
        event["operation_id"] = json!(operation_id);
        self.write_event_lossy(event);
        UsageToolRunGuard {
            context: self.clone(),
            tool,
            operation_id,
            started_at: Instant::now(),
            completed: false,
        }
    }

    fn record_tool_completed(
        &self,
        tool: &UsageTool,
        operation_id: &str,
        duration: Duration,
        status: UsageStatus,
    ) {
        let mut event = self.agent_event_base("tool_run_completed");
        event["operation_id"] = json!(operation_id);
        event["tool"] = tool.to_json();
        event["duration_ms"] = json!(duration_ms(duration));
        event["status"] = json!(status.as_str());
        add_time_range(&mut event, duration);
        self.write_event_lossy(event);
    }

    fn record_agent_completed(&self, duration: Duration, status: UsageStatus) {
        let mut event = self.agent_event_base("agent_run_completed");
        event["duration_ms"] = json!(duration_ms(duration));
        event["status"] = json!(status.as_str());
        add_time_range(&mut event, duration);
        self.write_event_lossy(event);

        if self.root_owner {
            let mut event = json!({
                "event_type": "root_run_completed",
                "timestamp": timestamp(),
                "root_run_id": self.root_run_id,
                "duration_ms": duration_ms(duration),
                "status": status.as_str(),
                "agent_run_id": self.agent_run_id,
            });
            add_time_range(&mut event, duration);
            self.write_event_lossy(event);
        }
    }

    fn child_env(&self, launched_by: UsageLaunchedBy) -> Vec<(&'static str, String)> {
        let mut env = vec![
            (
                crate::usage_store::TRACKING_ENV,
                if self.sink.tracking { "on" } else { "off" }.to_string(),
            ),
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
        self.write_event_to_sinks(event, true)
    }

    fn write_event_to_sinks(&self, mut event: Value, legacy_export: bool) -> Result<(), String> {
        event["event_id"] = json!(format!("cai_event_{}", Uuid::now_v7()));
        event["schema_version"] = json!(1);
        event["capture_incomplete"] = json!(self.sink.warned.load(Ordering::Relaxed));
        event["runtime_version"] = json!(self
            .agent
            .as_ref()
            .and_then(|agent| agent["runtime_version"].as_str())
            .unwrap_or(env!("CARGO_PKG_VERSION")));
        if let Some(revision) = self.agent.as_ref().and_then(|agent| {
            agent["package_revision"]
                .as_str()
                .or_else(|| agent["package"]["version"].as_str())
        }) {
            event["package_revision"] = json!(revision);
        }
        if self.sink.tracking {
            self.persist_history(event.clone());
        }
        if !legacy_export || self.sink.path.as_os_str().is_empty() {
            return Ok(());
        }
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

    fn persist_history(&self, event: Value) {
        let Ok(mut pending) = self.sink.pending.lock() else {
            return;
        };
        if pending.len() < 100 {
            pending.push(event);
        } else {
            crate::usage_store::mark_incomplete();
        }
        let result = crate::usage_store::open_database(true).and_then(|connection| {
            let connection = connection.ok_or("Usage database unavailable")?;
            let mut written = 0;
            for event in pending.iter() {
                match crate::usage_store::insert_event(&connection, event) {
                    Ok(_) => written += 1,
                    Err(error) => {
                        pending.drain(..written);
                        return Err(error);
                    }
                }
            }
            pending.clear();
            Ok(())
        });
        if let Err(error) = result {
            crate::usage_store::mark_incomplete();
            if !self.sink.warned.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "Usage capture incomplete: {error}. Agent execution continues; up to 100 pending metadata facts are retained until shutdown."
                );
            }
        }
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
        self.context.record_tool_completed(
            &self.tool,
            &self.operation_id,
            self.started_at.elapsed(),
            status,
        );
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
            ProviderKind::Mistral => "mistral",
            ProviderKind::Ollama => "ollama",
            ProviderKind::OpenAi => "openai",
            ProviderKind::TypeSafe => "typesafe",
            ProviderKind::Xai => "xai",
        },
        "profile": profile_name,
        "model": model,
        "auth_mode": auth_mode,
    })
}

fn usage_json(usage: &ProviderUsage) -> Value {
    let mut usage = usage.clone();
    usage.input_token_details = usage
        .input_token_details
        .as_ref()
        .and_then(crate::providers::runtime::sanitize_token_details);
    usage.output_token_details = usage
        .output_token_details
        .as_ref()
        .and_then(crate::providers::runtime::sanitize_token_details);
    to_json_without_nulls(&usage)
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

fn add_time_range(event: &mut Value, duration: Duration) {
    let end = OffsetDateTime::now_utc();
    event["timestamp_basis"] = json!("completion_clock_minus_monotonic_elapsed");
    event["ended_at"] = json!(end.format(&Rfc3339).ok());
    event["started_at"] = json!((end
        - time::Duration::try_from(duration).unwrap_or(time::Duration::ZERO))
    .format(&Rfc3339)
    .ok());
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

        let attempt = context.start_provider_request(
            ProviderKind::OpenAi,
            Some("openai-account"),
            "openai_account",
            "gpt-5",
            UsageStep {
                kind: "agent_inference",
                action: None,
                step_index: None,
            },
        );
        context.record_provider_request(UsageProviderRequest {
            attempt: &attempt,
            provider: ProviderKind::OpenAi,
            profile_name: Some("openai-account"),
            auth_mode: "openai_account",
            model: "gpt-5",
            resolved_model: None,
            provider_request_id: None,
            finish_reason: None,
            step: UsageStep {
                kind: "agent_inference",
                action: None,
                step_index: None,
            },
            usage: Some(&ProviderUsage {
                total_tokens_source: Some("reported".to_string()),
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
        assert!(!contents.contains("provider_request_started"));
        let db = crate::usage_store::open_database(false).unwrap().unwrap();
        let raw: String = db.query_row("SELECT record_json FROM usage_events WHERE root_run_id=?1 AND event_type='provider_request_started'", [&context.root_run_id], |row|row.get(0)).unwrap();
        let started: Value = serde_json::from_str(&raw).unwrap();
        let completed: Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(started["event_type"], "provider_request_started");
        for field in ["attempt_id", "operation_id", "started_at"] {
            assert_eq!(started[field], completed[field]);
        }
        assert!(started.get("usage").is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn failed_provider_event_uses_redacted_error_metadata() {
        let path = std::env::temp_dir().join(format!("cargo-ai-usage-{}.jsonl", Uuid::now_v7()));
        let (context, mut guard) =
            UsageLogContext::from_runtime(Some(path.to_str().unwrap()), 0, None)
                .expect("usage log should initialize")
                .expect("usage log should be enabled");

        let attempt = context.start_provider_request(
            ProviderKind::Ollama,
            None,
            "none",
            "llama3.2",
            UsageStep {
                kind: "agent_inference",
                action: None,
                step_index: None,
            },
        );
        context.record_provider_request(UsageProviderRequest {
            attempt: &attempt,
            provider: ProviderKind::Ollama,
            profile_name: None,
            auth_mode: "none",
            model: "llama3.2",
            resolved_model: None,
            provider_request_id: None,
            finish_reason: None,
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

#[cfg(test)]
mod automatic_failure_tests {
    use super::*;
    struct Home {
        path: PathBuf,
        old: Option<std::ffi::OsString>,
        tracking: Option<std::ffi::OsString>,
    }
    impl Home {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("usage-runtime-{}", Uuid::now_v7()));
            let old = std::env::var_os("CARGO_AI_HOME");
            let tracking = std::env::var_os(crate::usage_store::TRACKING_ENV);
            unsafe {
                std::env::set_var("CARGO_AI_HOME", &path);
                std::env::remove_var(crate::usage_store::TRACKING_ENV);
            }
            Self {
                path,
                old,
                tracking,
            }
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            unsafe {
                if let Some(value) = &self.old {
                    std::env::set_var("CARGO_AI_HOME", value);
                } else {
                    std::env::remove_var("CARGO_AI_HOME");
                }
                if let Some(value) = &self.tracking {
                    std::env::set_var(crate::usage_store::TRACKING_ENV, value);
                } else {
                    std::env::remove_var(crate::usage_store::TRACKING_ENV);
                }
            }
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
    #[test]
    fn unavailable_history_preserves_success_and_explicit_export() {
        let home = Home::new();
        std::fs::create_dir_all(&home.path).unwrap();
        std::fs::write(home.path.join("usage"), "preserve").unwrap();
        let log = home.path.join("explicit.ndjson");
        let (_, mut guard) = UsageLogContext::from_runtime(Some(log.to_str().unwrap()), 0, None)
            .unwrap()
            .unwrap();
        guard.finish_success();
        let lines = std::fs::read_to_string(log).unwrap();
        assert!(lines.contains("root_run_completed"));
        assert!(lines.contains("success"));
        assert_eq!(
            std::fs::read_to_string(home.path.join("usage")).unwrap(),
            "preserve"
        );
        assert!(crate::usage_store::open_database(false).is_err());
    }
    #[test]
    fn explicit_export_with_optout_does_not_initialize_database() {
        let home = Home::new();
        std::fs::create_dir_all(&home.path).unwrap();
        unsafe {
            std::env::set_var(crate::usage_store::TRACKING_ENV, "off");
        }
        let log = home.path.join("explicit.ndjson");
        let (_, mut guard) = UsageLogContext::from_runtime(Some(log.to_str().unwrap()), 0, None)
            .unwrap()
            .unwrap();
        guard.finish_success();
        assert!(!home.path.join("usage").exists());
        assert_eq!(std::fs::read_to_string(log).unwrap().lines().count(), 4);
    }
    #[test]
    fn concurrent_writers_and_reader_preserve_facts_through_runtime_final_flush() {
        let _home = Home::new();
        let contexts: Vec<_> = (0..8)
            .map(|_| {
                UsageLogContext::from_runtime(None, 0, None)
                    .unwrap()
                    .unwrap()
            })
            .collect();
        let mut writer = crate::usage_store::open_database(true).unwrap().unwrap();
        let reader = crate::usage_store::open_database(false).unwrap().unwrap();
        let transaction = writer
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let phases = Arc::new(std::sync::Barrier::new(9));
        let threads: Vec<_> = contexts
            .into_iter()
            .enumerate()
            .map(|(writer_index, (context, guard))| {
                let phases = phases.clone();
                std::thread::spawn(move || {
                    let mut expected = Vec::new();
                    for event_index in 0..20 {
                        let event = json!({
                            "event_id": format!("concurrent-{writer_index}-{event_index}"),
                            "event_type": "provider_request_completed",
                            "timestamp": "2026-09-22T00:00:00Z",
                            "root_run_id": context.root_run_id,
                            "agent_run_id": context.agent_run_id,
                            "usage": {"input_tokens": writer_index, "output_tokens": event_index}
                        });
                        // Exercise the same bounded queue used by automatic capture,
                        // rather than requiring every individual SQLite write to win.
                        context.persist_history(event.clone());
                        expected.push(event);
                        if event_index == 0 {
                            let pending = context.sink.pending.lock().unwrap().len();
                            phases.wait();
                            phases.wait();
                            assert_eq!(pending, 1, "the held writer must exercise buffering");
                        }
                    }
                    (context, guard, expected)
                })
            })
            .collect();

        phases.wait();
        let initial_count: i64 = reader
            .query_row("SELECT COUNT(*) FROM usage_events", [], |row| row.get(0))
            .unwrap();
        drop(transaction);
        phases.wait();
        assert_eq!(
            initial_count, 16,
            "WAL readers retain the committed lifecycle facts while a writer holds the database"
        );
        assert!(crate::usage_store::incomplete());
        let live_count: i64 = reader
            .query_row("SELECT COUNT(*) FROM usage_events", [], |row| row.get(0))
            .unwrap();
        assert!((16..=176).contains(&live_count));
        let completed: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        let mut expected = std::collections::BTreeMap::new();
        for (context, mut guard, events) in completed {
            // Contention has ended. One ordinary run completion is the production
            // final-flush opportunity; no test-only insert retries are performed.
            guard.finish_success();
            assert!(context.sink.pending.lock().unwrap().is_empty());
            for event in events {
                expected.insert(event["event_id"].as_str().unwrap().to_owned(), event);
            }
        }
        let mut statement = reader.prepare("SELECT event_id, record_json FROM usage_events WHERE event_type='provider_request_completed'").unwrap();
        let records: Vec<(String, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(records.len(), 160);
        assert_eq!(expected.len(), 160);
        for (id, raw) in records {
            assert_eq!(
                serde_json::from_str::<Value>(&raw).unwrap(),
                expected.remove(&id).unwrap()
            );
        }
        assert!(expected.is_empty());
        let (total, unique): (i64, i64) = reader
            .query_row(
                "SELECT COUNT(*), COUNT(DISTINCT event_id) FROM usage_events",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((total, unique), (192, 192));
    }

    #[test]
    fn busy_pending_facts_flush_once_after_writer_releases() {
        let _home = Home::new();
        let mut db = crate::usage_store::open_database(true).unwrap().unwrap();
        let transaction = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let (context, mut guard) = UsageLogContext::from_runtime(None, 0, None)
            .unwrap()
            .unwrap();
        assert_eq!(context.sink.pending.lock().unwrap().len(), 2);
        assert!(crate::usage_store::incomplete());
        drop(transaction);
        guard.finish_success();
        assert!(context.sink.pending.lock().unwrap().is_empty());
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM usage_events", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            4
        );
        assert_eq!(
            db.query_row(
                "SELECT COUNT(DISTINCT event_id) FROM usage_events",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            4
        );
    }
}
