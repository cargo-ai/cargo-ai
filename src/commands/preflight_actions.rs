//! Action execution helpers for preflight/test flows.
use crate::config::adder::set_account_tokens;
use crate::config::loader::{config_path, find_profile, load_config};
use crate::config::schema::ProfileAuthMode;
use crate::credentials::openai_oauth;
use crate::credentials::store;
use crate::infra_api;
use jsonlogic::apply;
use std::collections::{BTreeMap, VecDeque};
use std::io::{self, IsTerminal, Write};
use std::path::{Component, Path};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const INFRA_BASE_URL: &str = "https://api.cargo-ai.org";
const AGENT_ACTION_DEPTH_ENV: &str = "CARGO_AI_AGENT_ACTION_DEPTH";
const AGENT_ACTION_MAX_DEPTH_ENV: &str = "CARGO_AI_AGENT_ACTION_MAX_DEPTH";
const AGENT_ACTION_MAX_RUNTIME_SECS_ENV: &str = "CARGO_AI_AGENT_MAX_RUNTIME_SECS";
const AGENT_ACTION_RUNTIME_STARTED_AT_MS_ENV: &str = "CARGO_AI_AGENT_RUNTIME_STARTED_AT_MS";
const AGENT_ACTION_RUNTIME_DEADLINE_MS_ENV: &str = "CARGO_AI_AGENT_RUNTIME_DEADLINE_MS";
const DEFAULT_AGENT_ACTION_MAX_RUNTIME_SECS: u64 = 600;
const SUPPORTED_FILE_EXTENSIONS_MESSAGE: &str = "pdf, docx, csv, xla, xlb, xlc, xlm, xls, xlsx, xlt, xlw, tsv, iif, doc, dot, odt, rtf, pot, ppa, pps, ppt, pptx, pwz, wiz";
const ACTION_LANE_OUTPUT_BUFFER_LIMIT: usize = 6;

tokio::task_local! {
    static ACTION_OUTPUT: ActionOutput;
}

#[derive(Clone)]
struct ActionOutput {
    inner: Arc<Mutex<ActionOutputState>>,
    live_refresh_stop: Arc<AtomicBool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActionOutputMode {
    AppendOnly,
    Live,
}

struct ActionOutputState {
    mode: ActionOutputMode,
    action_execution: crate::ActionExecutionMode,
    rendered_lines: usize,
    lanes: BTreeMap<usize, ActionLaneState>,
}

#[derive(Clone)]
struct ActionLaneState {
    action_name: String,
    status: ActionLaneStatus,
    lane_started_at: Option<Instant>,
    lane_finished_after: Option<Duration>,
    current_step: Option<String>,
    step_started_at: Option<Instant>,
    last_message: Option<String>,
    output_lines: VecDeque<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActionLaneStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Aborted,
    Notice,
    LogicError,
    Skipped,
}

impl ActionOutput {
    fn new(action_execution: crate::ActionExecutionMode) -> Self {
        Self::new_for_mode(
            action_execution,
            if should_use_live_action_dashboard() {
                ActionOutputMode::Live
            } else {
                ActionOutputMode::AppendOnly
            },
        )
    }

    fn new_for_mode(action_execution: crate::ActionExecutionMode, mode: ActionOutputMode) -> Self {
        let inner = Arc::new(Mutex::new(ActionOutputState {
            mode,
            action_execution,
            rendered_lines: 0,
            lanes: BTreeMap::new(),
        }));
        let live_refresh_stop = Arc::new(AtomicBool::new(false));
        maybe_spawn_live_action_refresh(inner.clone(), live_refresh_stop.clone(), mode);
        Self {
            inner,
            live_refresh_stop,
        }
    }

    fn print_execution_header(&self) {
        self.with_state(|state| {
            if state.mode == ActionOutputMode::AppendOnly {
                println!("{}", action_execution_header(state.action_execution));
            } else {
                render_live_dashboard(state);
            }
        });
    }

    fn action_started(&self, action_index: usize, action_name: &str) {
        self.with_state(|state| {
            let lane = ensure_lane_state(state, action_index, action_name);
            lane.status = ActionLaneStatus::Running;
            lane.lane_started_at = Some(Instant::now());
            lane.lane_finished_after = None;
            lane.last_message = Some("started".to_string());
            if state.mode == ActionOutputMode::AppendOnly {
                println!(
                    "{}",
                    format_action_line(action_index, action_name, "started")
                );
            } else {
                render_live_dashboard(state);
            }
        });
    }

    fn action_step_started(
        &self,
        action_index: usize,
        action_name: &str,
        step_kind: &str,
        step_number: usize,
        step_count: usize,
    ) {
        self.with_state(|state| {
            let lane = ensure_lane_state(state, action_index, action_name);
            lane.status = ActionLaneStatus::Running;
            lane.current_step = Some(format!("{}/{} {}", step_number, step_count, step_kind));
            lane.step_started_at = Some(Instant::now());
            lane.last_message = Some(waiting_message_for_step_kind(step_kind).to_string());
            if state.mode == ActionOutputMode::AppendOnly {
                println!(
                    "{}",
                    format_action_line(
                        action_index,
                        action_name,
                        format!(
                            "step {}/{} {} started; {}",
                            step_number,
                            step_count,
                            step_kind,
                            waiting_message_for_step_kind(step_kind)
                        )
                        .as_str(),
                    )
                );
            } else {
                render_live_dashboard(state);
            }
        });
    }

    fn action_line(&self, action_index: usize, action_name: &str, message: &str) {
        self.with_state(|state| {
            if state.mode == ActionOutputMode::AppendOnly {
                for line in split_action_output_lines(message) {
                    println!(
                        "{}",
                        format_action_line(action_index, action_name, line.as_str())
                    );
                }
                return;
            }

            let lane = ensure_lane_state(state, action_index, action_name);
            lane.last_message = compact_action_output_line(message);
            push_lane_output_message(lane, message);
            if lane.status == ActionLaneStatus::Pending {
                lane.status = inferred_lane_status(message);
            } else if lane.status == ActionLaneStatus::Running {
                lane.status = match inferred_lane_status(message) {
                    ActionLaneStatus::Notice => ActionLaneStatus::Running,
                    other => other,
                };
            }
            render_live_dashboard(state);
        });
    }

    fn action_success(&self, action_index: usize, action_name: &str, summary: &str) {
        self.with_state(|state| {
            let append_only = state.mode == ActionOutputMode::AppendOnly;
            let append_message = {
                let lane = ensure_lane_state(state, action_index, action_name);
                lane.status = ActionLaneStatus::Completed;
                lane.current_step = None;
                lane.step_started_at = None;
                lane.lane_finished_after =
                    lane.lane_started_at.map(|started_at| started_at.elapsed());
                lane.last_message = Some(format!("{}.", summary));
                if append_only {
                    Some(format!(
                        "{} in {}.",
                        summary,
                        format_elapsed_duration(
                            lane.lane_finished_after
                                .unwrap_or_else(|| Duration::from_secs(0))
                        )
                    ))
                } else {
                    None
                }
            };
            if let Some(message) = append_message {
                println!(
                    "{}",
                    format_action_line(action_index, action_name, message.as_str())
                );
            } else {
                render_live_dashboard(state);
            }
        });
    }

    fn action_failed(&self, action_index: usize, action_name: &str, error: &str) {
        self.with_state(|state| {
            let append_only = state.mode == ActionOutputMode::AppendOnly;
            let append_message = {
                let lane = ensure_lane_state(state, action_index, action_name);
                lane.status = ActionLaneStatus::Failed;
                lane.current_step = None;
                lane.step_started_at = None;
                lane.lane_finished_after =
                    lane.lane_started_at.map(|started_at| started_at.elapsed());
                lane.last_message = compact_action_output_line(error)
                    .map(|line| format!("failed: {}", line))
                    .or_else(|| Some("failed".to_string()));
                push_lane_output_message(lane, error);
                if append_only {
                    Some(format!(
                        "failed in {}.",
                        format_elapsed_duration(
                            lane.lane_finished_after
                                .unwrap_or_else(|| Duration::from_secs(0))
                        )
                    ))
                } else {
                    None
                }
            };
            if let Some(message) = append_message {
                println!(
                    "{}",
                    format_action_line(action_index, action_name, message.as_str())
                );
            } else {
                render_live_dashboard(state);
            }
        });
    }

    fn action_aborted(&self, action_index: usize, action_name: &str, error: &str) {
        self.with_state(|state| {
            let append_only = state.mode == ActionOutputMode::AppendOnly;
            let append_message = {
                let lane = ensure_lane_state(state, action_index, action_name);
                lane.status = ActionLaneStatus::Aborted;
                lane.current_step = None;
                lane.step_started_at = None;
                lane.lane_finished_after =
                    lane.lane_started_at.map(|started_at| started_at.elapsed());
                lane.last_message = compact_action_output_line(error)
                    .map(|line| format!("abort requested: {}", line))
                    .or_else(|| Some("abort requested".to_string()));
                push_lane_output_message(lane, error);
                if append_only {
                    Some(format!(
                        "abort requested in {}: {}",
                        format_elapsed_duration(
                            lane.lane_finished_after
                                .unwrap_or_else(|| Duration::from_secs(0))
                        ),
                        error
                    ))
                } else {
                    None
                }
            };
            if let Some(message) = append_message {
                println!(
                    "{}",
                    format_action_line(action_index, action_name, message.as_str())
                );
            } else {
                render_live_dashboard(state);
            }
        });
    }

    fn action_stopped_by_abort(&self, action_index: usize, action_name: &str) {
        self.with_state(|state| {
            if state.mode == ActionOutputMode::AppendOnly {
                return;
            }

            let lane = ensure_lane_state(state, action_index, action_name);
            lane.status = ActionLaneStatus::Aborted;
            lane.current_step = None;
            lane.step_started_at = None;
            lane.lane_finished_after = lane.lane_started_at.map(|started_at| started_at.elapsed());
            lane.last_message = Some("stopped after invocation abort.".to_string());
            render_live_dashboard(state);
        });
    }

    fn finish(&self) {
        self.live_refresh_stop.store(true, Ordering::Relaxed);
        self.with_state(|state| {
            if state.mode == ActionOutputMode::Live {
                render_live_dashboard(state);
                let _ = writeln!(io::stdout());
                let _ = io::stdout().flush();
                state.rendered_lines = 0;
            }
        });
    }

    #[cfg(test)]
    fn snapshot_lines_for_test(&self) -> Vec<String> {
        self.inner
            .lock()
            .expect("action output lock should succeed")
            .snapshot_lines()
    }

    fn with_state(&self, update: impl FnOnce(&mut ActionOutputState)) {
        let mut state = self
            .inner
            .lock()
            .expect("action output lock should succeed");
        update(&mut state);
    }
}

impl ActionOutputState {
    fn snapshot_lines(&self) -> Vec<String> {
        let mut lines = vec![action_execution_header(self.action_execution).to_string()];

        for (lane_index, lane) in &self.lanes {
            lines.push(String::new());
            lines.push(format!(
                "{} {}",
                action_lane_prefix(*lane_index, lane.action_name.as_str()),
                lane_status_label(lane)
            ));
            lines.push(format!("  step: {}", lane_step_label(lane)));
            lines.push(format!(
                "  last: {}",
                lane.last_message.as_deref().unwrap_or("-")
            ));
            if !lane.output_lines.is_empty() {
                lines.push("  output:".to_string());
                for line in &lane.output_lines {
                    lines.push(format!("    {}", line));
                }
            }
        }

        lines
    }
}

impl ActionLaneStatus {
    fn display_name(self) -> &'static str {
        match self {
            ActionLaneStatus::Pending => "pending",
            ActionLaneStatus::Running => "running",
            ActionLaneStatus::Completed => "completed",
            ActionLaneStatus::Failed => "failed",
            ActionLaneStatus::Aborted => "aborted",
            ActionLaneStatus::Notice => "notice",
            ActionLaneStatus::LogicError => "logic error",
            ActionLaneStatus::Skipped => "skipped",
        }
    }
}

fn should_use_live_action_dashboard() -> bool {
    io::stdout().is_terminal()
        && std::env::var("TERM")
            .map(|term| term != "dumb")
            .unwrap_or(true)
        && std::env::var_os("CI").is_none()
}

fn ensure_lane_state<'a>(
    state: &'a mut ActionOutputState,
    action_index: usize,
    action_name: &str,
) -> &'a mut ActionLaneState {
    state
        .lanes
        .entry(action_index)
        .or_insert_with(|| ActionLaneState {
            action_name: action_name.to_string(),
            status: ActionLaneStatus::Pending,
            lane_started_at: None,
            lane_finished_after: None,
            current_step: None,
            step_started_at: None,
            last_message: None,
            output_lines: VecDeque::new(),
        })
}

fn maybe_spawn_live_action_refresh(
    inner: Arc<Mutex<ActionOutputState>>,
    stop: Arc<AtomicBool>,
    mode: ActionOutputMode,
) {
    if mode != ActionOutputMode::Live || cfg!(test) {
        return;
    }

    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };

    handle.spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            interval.tick().await;
            if stop.load(Ordering::Relaxed) {
                break;
            }

            let mut state = inner.lock().expect("action output lock should succeed");
            if state.mode != ActionOutputMode::Live {
                break;
            }
            if state.lanes.values().any(|lane| {
                lane.status == ActionLaneStatus::Running && lane.step_started_at.is_some()
            }) {
                render_live_dashboard(&mut state);
            }
        }
    });
}

fn inferred_lane_status(message: &str) -> ActionLaneStatus {
    if message.starts_with("logic evaluation failed:") {
        ActionLaneStatus::LogicError
    } else if message.contains("no run steps matched") || message.contains("unsupported step kind")
    {
        ActionLaneStatus::Skipped
    } else {
        ActionLaneStatus::Notice
    }
}

fn split_action_output_lines(message: &str) -> Vec<String> {
    message
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn compact_action_output_line(message: &str) -> Option<String> {
    split_action_output_lines(message).into_iter().next()
}

fn push_lane_output_message(lane: &mut ActionLaneState, message: &str) {
    for line in split_action_output_lines(message) {
        if lane.output_lines.len() == ACTION_LANE_OUTPUT_BUFFER_LIMIT {
            lane.output_lines.pop_front();
        }
        lane.output_lines.push_back(line);
    }
}

fn lane_step_label(lane: &ActionLaneState) -> String {
    match lane.status {
        ActionLaneStatus::Completed => "✓ done".to_string(),
        ActionLaneStatus::Failed | ActionLaneStatus::LogicError => "✗ failed".to_string(),
        ActionLaneStatus::Aborted => "! aborted".to_string(),
        ActionLaneStatus::Skipped => "skipped".to_string(),
        _ => lane.current_step.clone().unwrap_or_else(|| "-".to_string()),
    }
}

fn lane_status_label(lane: &ActionLaneState) -> String {
    if let Some(elapsed) = lane_elapsed_duration(lane) {
        match lane.status {
            ActionLaneStatus::Running => {
                return format!("running · {}", format_elapsed_duration(elapsed));
            }
            ActionLaneStatus::Completed => {
                return format!("completed · {}", format_elapsed_duration(elapsed));
            }
            ActionLaneStatus::Failed | ActionLaneStatus::LogicError => {
                return format!("failed · {}", format_elapsed_duration(elapsed));
            }
            ActionLaneStatus::Aborted => {
                return format!("aborted · {}", format_elapsed_duration(elapsed));
            }
            _ => {}
        }
    }

    lane.status.display_name().to_string()
}

fn lane_elapsed_duration(lane: &ActionLaneState) -> Option<Duration> {
    lane.lane_finished_after
        .or_else(|| lane.lane_started_at.map(|started_at| started_at.elapsed()))
}

fn format_elapsed_duration(duration: Duration) -> String {
    format!("{}s", duration.as_secs())
}

fn waiting_message_for_step_kind(step_kind: &str) -> &'static str {
    if step_kind.eq_ignore_ascii_case("exec") {
        "waiting for command to finish..."
    } else if step_kind.eq_ignore_ascii_case("agent") {
        "waiting for child agent to finish..."
    } else if step_kind.eq_ignore_ascii_case("generate_image") {
        "waiting for provider response..."
    } else if step_kind.eq_ignore_ascii_case("email_me") {
        "waiting for mail response..."
    } else {
        "waiting for step to finish..."
    }
}

fn render_live_dashboard(state: &mut ActionOutputState) {
    if cfg!(test) {
        state.rendered_lines = state.snapshot_lines().len();
        return;
    }

    let snapshot = state.snapshot_lines();
    let mut stdout = io::stdout();

    if state.rendered_lines > 0 {
        let _ = write!(stdout, "\r");
        if state.rendered_lines > 1 {
            let _ = write!(stdout, "\x1b[{}A", state.rendered_lines - 1);
        }
        let _ = write!(stdout, "\x1b[J");
    }

    let _ = write!(stdout, "{}", snapshot.join("\n"));
    let _ = stdout.flush();
    state.rendered_lines = snapshot.len();
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct InvocationRuntimeBudget {
    pub(crate) max_runtime_secs: u64,
    pub(crate) started_at_ms: u64,
    pub(crate) deadline_ms: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct ActionProviderContext {
    pub(crate) provider: crate::providers::ProviderKind,
    pub(crate) model: String,
    pub(crate) url: String,
    pub(crate) token: String,
    pub(crate) inference_timeout_in_sec: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepExecutionOutcome {
    Completed,
    SoftFailureLogged,
    SuccessAlreadyPrinted,
}

#[derive(Debug)]
enum ActionExecutionResult {
    Completed(Vec<StepExecutionOutcome>),
    Failed(String),
    Aborted(String),
    StoppedByAbort,
}

#[derive(Debug, Clone)]
struct InvocationAbortSignal {
    inner: Arc<Mutex<InvocationAbortState>>,
}

#[derive(Debug, Clone)]
struct InvocationAbortRecord {
    action_index: usize,
    action_name: String,
    error: String,
}

#[derive(Debug, Default)]
struct InvocationAbortState {
    record: Option<InvocationAbortRecord>,
}

impl InvocationAbortSignal {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(InvocationAbortState::default())),
        }
    }

    fn is_triggered(&self) -> bool {
        self.inner
            .lock()
            .expect("abort signal lock should succeed")
            .record
            .is_some()
    }

    fn trigger(&self, action_index: usize, action_name: &str, error: &str) -> bool {
        let mut state = self.inner.lock().expect("abort signal lock should succeed");
        if state.record.is_none() {
            state.record = Some(InvocationAbortRecord {
                action_index,
                action_name: action_name.to_string(),
                error: error.to_string(),
            });
            true
        } else {
            false
        }
    }

    fn record(&self) -> Option<InvocationAbortRecord> {
        self.inner
            .lock()
            .expect("abort signal lock should succeed")
            .record
            .clone()
    }
}

#[derive(Debug, Clone)]
struct AccountAuth {
    access_token: String,
    refresh_token: Option<String>,
}

#[derive(Debug)]
enum RefreshAccessError {
    MissingRefreshToken,
    RequestFailed(String),
    MissingRefreshedToken(serde_json::Value),
}

/// Applies configured action rules to model output and executes matching steps.
pub(crate) async fn apply_actions(
    output: &crate::Output,
    actions: &[crate::Action],
    runtime_vars: &serde_json::Map<String, serde_json::Value>,
    named_inputs: &[crate::Input],
    action_execution: crate::ActionExecutionMode,
    action_execution_override: Option<crate::ActionExecutionMode>,
    provider_context: &ActionProviderContext,
    max_agent_depth: u32,
    runtime_budget: InvocationRuntimeBudget,
) -> Result<(), String> {
    ACTION_OUTPUT
        .scope(ActionOutput::new(action_execution), async move {
            let abort_signal = InvocationAbortSignal::new();
            let invocation_started_at = Instant::now();
            let data = match action_data_from_output(output, runtime_vars) {
                Ok(data) => data,
                Err(error) => {
                    eprintln!("❌ Failed to serialize output for action evaluation: {error}");
                    return Err(format!(
                        "Failed to serialize output for action evaluation: {error}"
                    ));
                }
            };
            let current_platform = current_action_platform();
            let named_input_lookup = named_input_lookup(named_inputs);
            print_action_execution_header(action_execution);
            let top_level_failures = match action_execution {
                crate::ActionExecutionMode::Sequential => {
                    apply_actions_sequential(
                        actions,
                        &data,
                        &named_input_lookup,
                        current_platform,
                        action_execution_override,
                        provider_context,
                        max_agent_depth,
                        runtime_budget,
                        &abort_signal,
                    )
                    .await?
                }
                crate::ActionExecutionMode::Parallel => {
                    apply_actions_parallel(
                        actions,
                        &data,
                        &named_input_lookup,
                        current_platform,
                        action_execution_override,
                        provider_context,
                        max_agent_depth,
                        runtime_budget,
                        &abort_signal,
                    )
                    .await?
                }
            };

            finish_action_output();

            if let Some(abort) = abort_signal.record() {
                return Err(format!(
                    "{}\n{}",
                    format_abort_summary(&abort),
                    root_run_abort_message(invocation_started_at.elapsed())
                ));
            }

            if let Some(message) = root_run_completion_message(invocation_started_at.elapsed()) {
                if top_level_failures.is_empty() {
                    println!("{message}");
                }
            }

            if top_level_failures.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "{}\n{}",
                    format_top_level_action_failures(&top_level_failures),
                    root_run_failure_message(invocation_started_at.elapsed())
                ))
            }
        })
        .await
}

async fn apply_actions_sequential(
    actions: &[crate::Action],
    data: &serde_json::Value,
    named_inputs: &BTreeMap<String, crate::Input>,
    current_platform: Option<&'static str>,
    action_execution_override: Option<crate::ActionExecutionMode>,
    provider_context: &ActionProviderContext,
    max_agent_depth: u32,
    runtime_budget: InvocationRuntimeBudget,
    abort_signal: &InvocationAbortSignal,
) -> Result<Vec<String>, String> {
    let mut top_level_failures = Vec::new();

    for (action_index, action) in actions.iter().enumerate() {
        if abort_signal.is_triggered() {
            break;
        }

        if !action_logic_matches(action_index, action, data) {
            continue;
        }

        let should_abort = collect_action_execution_result(
            action_index,
            action,
            run_matching_action_steps(
                action_index,
                action,
                data,
                named_inputs,
                current_platform,
                action_execution_override,
                provider_context,
                max_agent_depth,
                runtime_budget,
                abort_signal,
            )
            .await?,
            &mut top_level_failures,
        );

        if should_abort {
            break;
        }
    }

    Ok(top_level_failures)
}

async fn apply_actions_parallel(
    actions: &[crate::Action],
    data: &serde_json::Value,
    named_inputs: &BTreeMap<String, crate::Input>,
    current_platform: Option<&'static str>,
    action_execution_override: Option<crate::ActionExecutionMode>,
    provider_context: &ActionProviderContext,
    max_agent_depth: u32,
    runtime_budget: InvocationRuntimeBudget,
    abort_signal: &InvocationAbortSignal,
) -> Result<Vec<String>, String> {
    let mut matched_actions = Vec::new();
    let mut lane_tasks = Vec::new();
    let action_output = current_action_output();

    for (action_index, action) in actions.iter().enumerate() {
        if abort_signal.is_triggered() {
            break;
        }

        if !action_logic_matches(action_index, action, data) {
            continue;
        }

        matched_actions.push((action_index, action.clone()));

        let action_clone = action.clone();
        let data_clone = data.clone();
        let named_inputs_clone = named_inputs.clone();
        let provider_context_clone = provider_context.clone();
        let abort_signal_clone = abort_signal.clone();
        let action_output_clone = action_output.clone();

        lane_tasks.push(tokio::spawn(async move {
            let lane_future = async move {
                run_matching_action_steps(
                    action_index,
                    &action_clone,
                    &data_clone,
                    &named_inputs_clone,
                    current_platform,
                    action_execution_override,
                    &provider_context_clone,
                    max_agent_depth,
                    runtime_budget,
                    &abort_signal_clone,
                )
                .await
            };

            if let Some(output) = action_output_clone {
                ACTION_OUTPUT.scope(output, lane_future).await
            } else {
                lane_future.await
            }
        }));

        tokio::task::yield_now().await;
    }

    let mut top_level_failures = Vec::new();
    for ((action_index, action), task) in matched_actions.into_iter().zip(lane_tasks.into_iter()) {
        let result = task
            .await
            .map_err(|error| format!("parallel action task failed: {error}"))??;
        collect_action_execution_result(action_index, &action, result, &mut top_level_failures);
    }

    Ok(top_level_failures)
}

fn named_input_lookup(inputs: &[crate::Input]) -> BTreeMap<String, crate::Input> {
    let mut named = BTreeMap::new();
    for input in inputs {
        if let Some(name) = input.name.as_ref() {
            named.insert(name.clone(), input.clone());
        }
    }
    named
}

fn action_logic_matches(
    action_index: usize,
    action: &crate::Action,
    data: &serde_json::Value,
) -> bool {
    match apply(&action.logic, data) {
        Ok(result) => result.as_bool() == Some(true),
        Err(error) => {
            print_action_line(
                action_index,
                action.name.as_str(),
                format!("logic evaluation failed: {}", error).as_str(),
            );
            false
        }
    }
}

fn collect_action_execution_result(
    action_index: usize,
    action: &crate::Action,
    result: ActionExecutionResult,
    top_level_failures: &mut Vec<String>,
) -> bool {
    match result {
        ActionExecutionResult::Completed(outcomes) => {
            if let Some(summary) = action_completion_summary(&outcomes) {
                print_action_success(action_index, &action.name, summary);
            }
            false
        }
        ActionExecutionResult::Failed(error) => {
            note_action_failure(action_index, &action.name, &error);
            top_level_failures.push(format_action_failure(action_index, &action.name, &error));
            false
        }
        ActionExecutionResult::Aborted(error) => {
            note_action_abort(action_index, &action.name, &error);
            top_level_failures.push(format_action_failure(
                action_index,
                &action.name,
                format!("abort requested: {}", error).as_str(),
            ));
            true
        }
        ActionExecutionResult::StoppedByAbort => {
            note_action_stopped_by_abort(action_index, &action.name);
            false
        }
    }
}

async fn run_matching_action_steps(
    action_index: usize,
    action: &crate::Action,
    data: &serde_json::Value,
    named_inputs: &BTreeMap<String, crate::Input>,
    current_platform: Option<&'static str>,
    action_execution_override: Option<crate::ActionExecutionMode>,
    provider_context: &ActionProviderContext,
    max_agent_depth: u32,
    runtime_budget: InvocationRuntimeBudget,
    abort_signal: &InvocationAbortSignal,
) -> Result<ActionExecutionResult, String> {
    if abort_signal.is_triggered() {
        return Ok(ActionExecutionResult::StoppedByAbort);
    }

    let matching_steps = matching_run_steps(&action.run, current_platform);
    if matching_steps.is_empty() {
        print_action_line(
            action_index,
            action.name.as_str(),
            format!(
                "no run steps matched the current platform (current platform: {}).",
                current_platform.unwrap_or("unsupported")
            )
            .as_str(),
        );
        return Ok(ActionExecutionResult::Completed(Vec::new()));
    }

    print_action_start(action_index, &action.name);
    let single_step_action = matching_steps.len() == 1;
    let mut outcomes = Vec::with_capacity(matching_steps.len());
    let mut action_data = data.clone();

    let matching_step_count = matching_steps.len();
    for (step_index, step) in matching_steps.into_iter().enumerate() {
        if abort_signal.is_triggered() {
            return Ok(ActionExecutionResult::StoppedByAbort);
        }

        if !should_run_step(step, &action_data, &action.name)? {
            continue;
        }

        if abort_signal.is_triggered() {
            return Ok(ActionExecutionResult::StoppedByAbort);
        }

        note_action_step_started(
            action_index,
            action.name.as_str(),
            step.kind.as_str(),
            step_index + 1,
            matching_step_count,
        );

        let step_result = if step.kind.eq_ignore_ascii_case("exec") {
            run_exec_step(
                step,
                &action_data,
                action_index,
                &action.name,
                runtime_budget,
            )
            .await
            .map(|captured_output| (StepExecutionOutcome::Completed, captured_output))
        } else if step.kind.eq_ignore_ascii_case("email_me") {
            run_email_me_step(
                step,
                &action_data,
                action_index,
                &action.name,
                runtime_budget,
                single_step_action,
            )
            .await
            .map(|outcome| (outcome, None))
        } else if step.kind.eq_ignore_ascii_case("agent") {
            run_agent_step(
                step,
                &action_data,
                named_inputs,
                action_index,
                &action.name,
                action_execution_override,
                max_agent_depth,
                runtime_budget,
            )
            .await
            .map(|outcome| (outcome, None))
        } else if step.kind.eq_ignore_ascii_case("generate_image") {
            run_generate_image_step(
                step,
                &action_data,
                action_index,
                &action.name,
                provider_context,
                runtime_budget,
            )
            .await
            .map(|outcome| (outcome, None))
        } else {
            print_action_line(
                action_index,
                action.name.as_str(),
                format!("unsupported step kind '{}'; skipping step.", step.kind).as_str(),
            );
            outcomes.push(StepExecutionOutcome::SoftFailureLogged);
            continue;
        };

        match step_result {
            Ok((outcome, captured_output)) => {
                if let Some((name, value)) = captured_output {
                    insert_action_output_variable(
                        &mut action_data,
                        name.as_str(),
                        value,
                        action.name.as_str(),
                    )?;
                }
                insert_step_status_variable(
                    &mut action_data,
                    step,
                    "succeeded",
                    action.name.as_str(),
                )?;
                outcomes.push(outcome);
            }
            Err(error) => {
                insert_step_status_variable(
                    &mut action_data,
                    step,
                    "failed",
                    action.name.as_str(),
                )?;
                insert_step_error_variable(
                    &mut action_data,
                    step,
                    error.as_str(),
                    action.name.as_str(),
                )?;

                match step_failure_mode(step) {
                    crate::FailureMode::Continue => {
                        print_action_line(action_index, action.name.as_str(), error.as_str());
                        outcomes.push(StepExecutionOutcome::SoftFailureLogged);
                    }
                    crate::FailureMode::Stop => {
                        return Ok(ActionExecutionResult::Failed(error));
                    }
                    crate::FailureMode::Abort => {
                        abort_signal.trigger(action_index, action.name.as_str(), error.as_str());
                        return Ok(ActionExecutionResult::Aborted(error));
                    }
                }
            }
        }
    }

    Ok(ActionExecutionResult::Completed(outcomes))
}

fn action_data_from_output(
    output: &crate::Output,
    runtime_vars: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value, serde_json::Error> {
    let mut data = serde_json::to_value(output)?;
    if let Some(object) = data.as_object_mut() {
        object.insert(
            "runtime".to_string(),
            serde_json::Value::Object(runtime_vars.clone()),
        );
    }
    Ok(data)
}

async fn run_exec_step(
    step: &crate::RunStep,
    data: &serde_json::Value,
    action_index: usize,
    action_name: &str,
    runtime_budget: InvocationRuntimeBudget,
) -> Result<Option<(String, String)>, String> {
    let program = step.program.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' exec step is missing required `program`.",
            action_name
        )
    })?;

    let resolved_args = resolve_run_args(&step.args, data, action_name)
        .map_err(|error| format!("Action '{}': {error}", action_name))?;

    let remaining = remaining_runtime_duration(
        runtime_budget,
        &format!("before starting command '{}'", program),
    )
    .map_err(|context| {
        action_runtime_timeout_message(action_name, runtime_budget, context.as_str())
    })?;

    if let Some(output_variable) = step.output_variable.as_deref() {
        let child = tokio::process::Command::new(program)
            .args(&resolved_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("{action_name}: failed to execute command: {error}."))?;

        match tokio::time::timeout(remaining, child.wait_with_output()).await {
            Ok(Ok(output)) if output.status.success() => {
                emit_exec_output_lines(
                    action_index,
                    action_name,
                    &output.stdout,
                    &output.stderr,
                    false,
                );
                let captured_output = String::from_utf8_lossy(&output.stdout)
                    .trim_end_matches(['\r', '\n'])
                    .to_string();
                print_action_line(
                    action_index,
                    action_name,
                    format!("stored exec output in variable '{}'.", output_variable).as_str(),
                );
                Ok(Some((output_variable.to_string(), captured_output)))
            }
            Ok(Ok(output)) => {
                emit_exec_output_lines(
                    action_index,
                    action_name,
                    &output.stdout,
                    &output.stderr,
                    true,
                );
                Err(format!(
                    "Action '{}' exec step command '{}' exited with status {}.",
                    action_name, program, output.status
                ))
            }
            Ok(Err(error)) => Err(format!(
                "Action '{}' exec step failed while waiting for command '{}': {}",
                action_name, program, error
            )),
            Err(_) => Err(action_runtime_timeout_message(
                action_name,
                runtime_budget,
                &format!("while waiting for command '{}'", program),
            )),
        }
    } else {
        let child = tokio::process::Command::new(program)
            .args(&resolved_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("{action_name}: failed to execute command: {error}."))?;

        match tokio::time::timeout(remaining, child.wait_with_output()).await {
            Ok(Ok(output)) if output.status.success() => {
                emit_exec_output_lines(
                    action_index,
                    action_name,
                    &output.stdout,
                    &output.stderr,
                    true,
                );
                Ok(None)
            }
            Ok(Ok(output)) => {
                emit_exec_output_lines(
                    action_index,
                    action_name,
                    &output.stdout,
                    &output.stderr,
                    true,
                );
                Err(format!(
                    "Action '{}' exec step command '{}' exited with status {}.",
                    action_name, program, output.status
                ))
            }
            Ok(Err(error)) => Err(format!(
                "Action '{}' exec step failed while waiting for command '{}': {}",
                action_name, program, error
            )),
            Err(_) => Err(action_runtime_timeout_message(
                action_name,
                runtime_budget,
                &format!("while waiting for command '{}'", program),
            )),
        }
    }
}

fn emit_exec_output_lines(
    action_index: usize,
    action_name: &str,
    stdout: &[u8],
    stderr: &[u8],
    include_stdout: bool,
) {
    if include_stdout {
        emit_action_output_bytes(action_index, action_name, stdout);
    }
    emit_action_output_bytes(action_index, action_name, stderr);
}

fn emit_action_output_bytes(action_index: usize, action_name: &str, bytes: &[u8]) {
    let rendered = String::from_utf8_lossy(bytes);
    for line in split_action_output_lines(rendered.as_ref()) {
        print_action_line(action_index, action_name, line.as_str());
    }
}

fn insert_action_output_variable(
    data: &mut serde_json::Value,
    name: &str,
    value: String,
    action_name: &str,
) -> Result<(), String> {
    insert_action_string_variable(data, name, value, action_name)
}

fn insert_action_string_variable(
    data: &mut serde_json::Value,
    name: &str,
    value: String,
    action_name: &str,
) -> Result<(), String> {
    let Some(object) = data.as_object_mut() else {
        return Err(format!(
            "Action '{}' could not store captured variable '{}' because the action data context is not an object.",
            action_name, name
        ));
    };

    object.insert(name.to_string(), serde_json::Value::String(value));
    Ok(())
}

fn insert_step_status_variable(
    data: &mut serde_json::Value,
    step: &crate::RunStep,
    status: &str,
    action_name: &str,
) -> Result<(), String> {
    let Some(name) = step.status_variable.as_deref() else {
        return Ok(());
    };

    insert_action_string_variable(data, name, status.to_string(), action_name)
}

fn insert_step_error_variable(
    data: &mut serde_json::Value,
    step: &crate::RunStep,
    error: &str,
    action_name: &str,
) -> Result<(), String> {
    let Some(name) = step.error_variable.as_deref() else {
        return Ok(());
    };

    insert_action_string_variable(data, name, error.to_string(), action_name)
}

fn step_failure_mode(step: &crate::RunStep) -> crate::FailureMode {
    step.failure_mode
        .clone()
        .unwrap_or(crate::FailureMode::Stop)
}

fn should_run_step(
    step: &crate::RunStep,
    data: &serde_json::Value,
    action_name: &str,
) -> Result<bool, String> {
    let Some(condition) = step.when.as_ref() else {
        return Ok(true);
    };

    apply(condition, data)
        .map(|result| result.as_bool() == Some(true))
        .map_err(|error| {
            format!(
                "Action '{}' failed to evaluate step `when` for kind '{}': {}",
                action_name, step.kind, error
            )
        })
}

async fn run_email_me_step(
    step: &crate::RunStep,
    data: &serde_json::Value,
    action_index: usize,
    action_name: &str,
    runtime_budget: InvocationRuntimeBudget,
    single_step_action: bool,
) -> Result<StepExecutionOutcome, String> {
    let subject_parts = step.subject.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' email_me step is missing required `subject`.",
            action_name
        )
    })?;
    let text_parts = step.text.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' email_me step is missing required `text`.",
            action_name
        )
    })?;

    let subject = resolve_string_parts(subject_parts, data, action_name, "subject")?;
    let text = resolve_string_parts(text_parts, data, action_name, "text")?;

    let remaining =
        remaining_runtime_duration(runtime_budget, "before sending email").map_err(|context| {
            action_runtime_timeout_message(action_name, runtime_budget, context.as_str())
        })?;

    let response = tokio::time::timeout(remaining, async {
        let auth = load_account_auth()?;
        let access_token_owned = auth.access_token;
        let refresh_token = auth.refresh_token;

        let mut response = infra_api::account::send_mail::send_test_mail(
            INFRA_BASE_URL,
            access_token_owned.as_str(),
            subject.as_str(),
            text.as_str(),
        )
        .await
        .map_err(|error| format!("Request failed: {error:?}"))?;

        let is_expired_error = response
            .get("type")
            .and_then(|v| v.as_str())
            .map(|t| t == "access_token_expired")
            .unwrap_or(false);

        if is_expired_error {
            response = match refresh_access_token_for_retry(
                access_token_owned.as_str(),
                refresh_token.as_deref(),
            )
            .await
            {
                Err(RefreshAccessError::MissingRefreshToken) => {
                    return Err(
                        "Access token expired, and no refresh token exists in credential store. Run `cargo ai account status` or re-confirm account."
                            .to_string(),
                    );
                }
                Err(RefreshAccessError::RequestFailed(error)) => {
                    return Err(format!("Request failed while refreshing session: {error}"));
                }
                Err(RefreshAccessError::MissingRefreshedToken(refresh_response)) => {
                    return Err(
                        format_backend_error_message(&refresh_response).unwrap_or_else(|| {
                            "Session refresh did not return a new access token. Cannot retry email_me action."
                                .to_string()
                        }),
                    );
                }
                Ok((retry_access_token, refreshed_expires_in)) => {
                    if let Some(rt) = refresh_token.as_deref() {
                        persist_refreshed_access_token(
                            retry_access_token.as_str(),
                            rt,
                            refreshed_expires_in,
                        );
                    }

                    infra_api::account::send_mail::send_test_mail(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        subject.as_str(),
                        text.as_str(),
                    )
                    .await
                    .map_err(|error| format!("Request failed after session refresh: {error:?}"))?
                }
            };
        }

        let succeeded = response
            .get("status")
            .and_then(|v| v.as_str())
            .map(|status| status.eq_ignore_ascii_case("success"))
            .unwrap_or(false);

        if succeeded {
            Ok(response)
        } else {
            Err(format_backend_error_message(&response).unwrap_or_else(|| {
                format!(
                    "Action '{}' email_me request failed.\n{}",
                    action_name,
                    pretty_backend_json(&response)
                )
            }))
        }
    })
    .await;

    let response = match response {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => return Err(error),
        Err(_) => {
            return Err(action_runtime_timeout_message(
                action_name,
                runtime_budget,
                "while sending email",
            ));
        }
    };

    if single_step_action {
        print_action_success(action_index, action_name, "email sent");
    }
    for line in render_backend_ui_or_json_lines(&response) {
        print_action_line(action_index, action_name, line.as_str());
    }
    Ok(if single_step_action {
        StepExecutionOutcome::SuccessAlreadyPrinted
    } else {
        StepExecutionOutcome::Completed
    })
}

async fn run_generate_image_step(
    step: &crate::RunStep,
    data: &serde_json::Value,
    action_index: usize,
    action_name: &str,
    provider_context: &ActionProviderContext,
    runtime_budget: InvocationRuntimeBudget,
) -> Result<StepExecutionOutcome, String> {
    let step_profile_context =
        resolve_generate_image_step_profile_context(step.profile.as_ref(), data, action_name)
            .await?;
    let effective_provider_context = step_profile_context.as_ref().unwrap_or(provider_context);

    if effective_provider_context.provider != crate::providers::ProviderKind::OpenAi {
        return Err(format!(
            "Action '{}' generate_image step requires `--server openai`; current server is {}.",
            action_name,
            effective_provider_context.provider.display_name()
        ));
    }

    let model = resolve_generate_image_model(
        step.model.as_ref(),
        data,
        action_name,
        step_profile_context.as_ref(),
        provider_context,
    )?;

    if effective_provider_context
        .url
        .contains("chatgpt.com/backend-api/codex")
        && model.starts_with("gpt-image")
    {
        return Err(format!(
            "Action '{}' generate_image step uses OpenAI account transport, so `model` must be a tool-capable mainline model such as `gpt-5.2`, not '{}'.",
            action_name, model
        ));
    }
    let prompt_parts = step.prompt.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' generate_image step is missing required `prompt`.",
            action_name
        )
    })?;
    let path_parts = step.path.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' generate_image step is missing required `path`.",
            action_name
        )
    })?;

    let prompt = resolve_string_parts(prompt_parts, data, action_name, "prompt")
        .map_err(|error| format!("Action '{}': {error}", action_name))?;
    let output_path = resolve_string_parts(path_parts, data, action_name, "path")
        .map_err(|error| format!("Action '{}': {error}", action_name))?;
    let output_format = generated_image_output_format(output_path.as_str(), action_name)?;

    let remaining = remaining_runtime_duration(
        runtime_budget,
        &format!("before starting image generation with model '{}'", model),
    )
    .map_err(|context| {
        action_runtime_timeout_message(action_name, runtime_budget, context.as_str())
    })?;

    let image_bytes = match tokio::time::timeout(
        remaining,
        crate::providers::send_openai_image_request(
            &effective_provider_context.url,
            &model,
            prompt.as_str(),
            effective_provider_context.inference_timeout_in_sec,
            &effective_provider_context.token,
            output_format,
        ),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => {
            let mut lines = vec![format!(
                "Action '{}' generate_image step failed.",
                action_name
            )];
            lines.extend(crate::providers::provider_error_messages(&error));
            return Err(lines.join("\n"));
        }
        Err(_) => {
            return Err(action_runtime_timeout_message(
                action_name,
                runtime_budget,
                "while waiting for image generation",
            ));
        }
    };

    let output_path_ref = Path::new(output_path.as_str());
    validate_generated_image_output_path(output_path_ref, action_name)?;
    if let Some(parent) = output_path_ref.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "Action '{}' failed to create image output directory '{}': {}",
                    action_name,
                    parent.display(),
                    error
                )
            })?;
        }
    }

    std::fs::write(output_path_ref, image_bytes).map_err(|error| {
        format!(
            "Action '{}' failed to write generated image '{}': {}",
            action_name,
            output_path_ref.display(),
            error
        )
    })?;

    print_action_line(
        action_index,
        action_name,
        format!("wrote generated image to '{}'.", output_path_ref.display()).as_str(),
    );
    Ok(StepExecutionOutcome::Completed)
}

fn resolve_step_profile_name(
    profile: Option<&crate::RunArg>,
    data: &serde_json::Value,
    action_name: &str,
    step_kind: &str,
) -> Result<Option<String>, String> {
    let Some(profile) = profile else {
        return Ok(None);
    };

    let profile_name = match profile {
        crate::RunArg::Literal(literal) => literal.clone(),
        crate::RunArg::Variable(variable) => {
            let Some(value) = lookup_action_variable(data, variable) else {
                return Err(format!(
                    "Action '{}' {} `profile` references missing variable '{}'.",
                    action_name, step_kind, variable
                ));
            };

            match value {
                serde_json::Value::String(text) => text.clone(),
                serde_json::Value::Bool(_) => {
                    return Err(format!(
                        "Action '{}' {} `profile` must resolve to a string, found boolean for variable '{}'.",
                        action_name, step_kind, variable
                    ));
                }
                serde_json::Value::Number(_) => {
                    return Err(format!(
                        "Action '{}' {} `profile` must resolve to a string, found number for variable '{}'.",
                        action_name, step_kind, variable
                    ));
                }
                serde_json::Value::Array(_) => {
                    return Err(format!(
                        "Action '{}' {} `profile` must resolve to a string, found array for variable '{}'.",
                        action_name, step_kind, variable
                    ));
                }
                serde_json::Value::Object(_) => {
                    return Err(format!(
                        "Action '{}' {} `profile` must resolve to a string, found object for variable '{}'.",
                        action_name, step_kind, variable
                    ));
                }
                serde_json::Value::Null => {
                    return Err(format!(
                        "Action '{}' {} `profile` must resolve to a string, found null for variable '{}'.",
                        action_name, step_kind, variable
                    ));
                }
            }
        }
    };

    if profile_name.trim().is_empty() {
        return Err(format!(
            "Action '{}' {} `profile` must resolve to a non-empty string.",
            action_name, step_kind
        ));
    }

    Ok(Some(profile_name))
}

fn resolve_profile_api_token_for_action_step(
    profile: &crate::config::schema::Profile,
) -> Result<String, String> {
    match store::load_profile_token(&profile.name) {
        Ok(Some(token)) if !token.trim().is_empty() => Ok(token),
        Ok(Some(_)) | Ok(None) => profile
            .token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                format!(
                    "Missing API token for profile '{}'. Use `cargo ai profile set {} --token <TOKEN> --auth api_key`.",
                    profile.name, profile.name
                )
            }),
        Err(error) => Err(format!(
            "Failed to load profile token for '{}': {error}",
            profile.name
        )),
    }
}

async fn resolve_generate_image_step_profile_context(
    profile: Option<&crate::RunArg>,
    data: &serde_json::Value,
    action_name: &str,
) -> Result<Option<ActionProviderContext>, String> {
    let Some(profile_name) =
        resolve_step_profile_name(profile, data, action_name, "generate_image")?
    else {
        return Ok(None);
    };

    let config_file = config_path();
    let Some(config) = load_config() else {
        return Err(format!(
            "Action '{}' generate_image step references profile '{}', but no Cargo AI config was found at '{}'.",
            action_name,
            profile_name,
            config_file.display()
        ));
    };

    let Some(profile) = find_profile(&config, &profile_name) else {
        return Err(format!(
            "Action '{}' generate_image step references unknown profile '{}'.",
            action_name, profile_name
        ));
    };

    let provider = crate::providers::ProviderKind::from_server_value(profile.server.as_str())
        .ok_or_else(|| {
            format!(
                "Action '{}' generate_image step profile '{}' uses unsupported server '{}'.",
                action_name, profile.name, profile.server
            )
        })?;

    if provider != crate::providers::ProviderKind::OpenAi {
        return Err(format!(
            "Action '{}' generate_image step profile '{}' uses server '{}', but generate_image requires openai.",
            action_name, profile.name, profile.server
        ));
    }

    let mut url = profile.url.clone().unwrap_or_default();
    let token = match profile.auth_mode {
        ProfileAuthMode::ApiKey => resolve_profile_api_token_for_action_step(profile)?,
        ProfileAuthMode::OpenaiAccount => {
            url = openai_oauth::OPENAI_ACCOUNT_RESPONSES_URL.to_string();
            openai_oauth::resolve_session_for_runtime()
                .await
                .map(|session| session.access_token)?
        }
        ProfileAuthMode::None => {
            return Err(format!(
                "Action '{}' generate_image step profile '{}' auth mode is '{}'. Set it to '{}' or '{}' before using it here.",
                action_name,
                profile.name,
                ProfileAuthMode::None.as_str(),
                ProfileAuthMode::ApiKey.as_str(),
                ProfileAuthMode::OpenaiAccount.as_str()
            ));
        }
    };

    if url.trim().is_empty() {
        url = provider.default_url().to_string();
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!(
            "Action '{}' generate_image step profile '{}' produced invalid URL '{}'. Use an absolute URL beginning with `http://` or `https://`.",
            action_name, profile.name, url
        ));
    }

    Ok(Some(ActionProviderContext {
        provider,
        model: profile.model.clone(),
        url,
        token,
        inference_timeout_in_sec: profile.timeout_in_sec,
    }))
}

fn resolve_generate_image_model(
    model: Option<&crate::RunArg>,
    data: &serde_json::Value,
    action_name: &str,
    step_profile_context: Option<&ActionProviderContext>,
    provider_context: &ActionProviderContext,
) -> Result<String, String> {
    let Some(model) = model else {
        if let Some(step_profile_context) = step_profile_context {
            if !step_profile_context.model.trim().is_empty() {
                return Ok(step_profile_context.model.clone());
            }
        }
        if provider_context.model.trim().is_empty() {
            return Err(format!(
                "Action '{}' generate_image step omitted `model`, and no effective invocation model is configured. Set `generate_image.model`, pass `--model`, or configure a profile model.",
                action_name
            ));
        }
        return Ok(provider_context.model.clone());
    };

    match model {
        crate::RunArg::Literal(literal) => {
            if literal.trim().is_empty() {
                return Err(format!(
                    "Action '{}' generate_image `model` must resolve to a non-empty string.",
                    action_name
                ));
            }
            Ok(literal.clone())
        }
        crate::RunArg::Variable(variable) => {
            let Some(value) = lookup_action_variable(data, variable) else {
                return Err(format!(
                    "Action '{}' generate_image `model` references missing variable '{}'.",
                    action_name, variable
                ));
            };

            match value {
                serde_json::Value::String(text) if !text.trim().is_empty() => Ok(text.clone()),
                serde_json::Value::String(_) => Err(format!(
                    "Action '{}' generate_image `model` resolved to an empty string.",
                    action_name
                )),
                serde_json::Value::Bool(_) => Err(format!(
                    "Action '{}' generate_image `model` must resolve to a string, found boolean for variable '{}'.",
                    action_name, variable
                )),
                serde_json::Value::Number(_) => Err(format!(
                    "Action '{}' generate_image `model` must resolve to a string, found number for variable '{}'.",
                    action_name, variable
                )),
                serde_json::Value::Array(_) => Err(format!(
                    "Action '{}' generate_image `model` must resolve to a string, found array for variable '{}'.",
                    action_name, variable
                )),
                serde_json::Value::Object(_) => Err(format!(
                    "Action '{}' generate_image `model` must resolve to a string, found object for variable '{}'.",
                    action_name, variable
                )),
                serde_json::Value::Null => Err(format!(
                    "Action '{}' generate_image `model` must resolve to a string, found null for variable '{}'.",
                    action_name, variable
                )),
            }
        }
    }
}

async fn run_agent_step(
    step: &crate::RunStep,
    data: &serde_json::Value,
    named_inputs: &BTreeMap<String, crate::Input>,
    action_index: usize,
    action_name: &str,
    action_execution_override: Option<crate::ActionExecutionMode>,
    max_agent_depth: u32,
    runtime_budget: InvocationRuntimeBudget,
) -> Result<StepExecutionOutcome, String> {
    let agent = step.agent.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' agent step is missing required `agent`.",
            action_name
        )
    })?;

    let current_depth = current_agent_action_depth();
    validate_agent_action_depth(current_depth, max_agent_depth, action_name)?;

    validate_agent_step_target(agent, action_name)?;
    let agent_path = Path::new(agent);
    if !agent_path.exists() {
        return Err(format!(
            "Action '{}' agent step target '{}' was not found relative to the current working directory.",
            action_name, agent
        ));
    }

    let mut command = tokio::process::Command::new(agent_path);
    if let Some(action_execution_override) = action_execution_override {
        command.arg("--action-execution");
        command.arg(match action_execution_override {
            crate::ActionExecutionMode::Sequential => "sequential",
            crate::ActionExecutionMode::Parallel => "parallel",
        });
    }
    if let Some(profile_name) =
        resolve_step_profile_name(step.profile.as_ref(), data, action_name, "agent")?
    {
        let config_file = config_path();
        let Some(config) = load_config() else {
            return Err(format!(
                "Action '{}' agent step references profile '{}', but no Cargo AI config was found at '{}'.",
                action_name,
                profile_name,
                config_file.display()
            ));
        };
        if find_profile(&config, &profile_name).is_none() {
            return Err(format!(
                "Action '{}' agent step references unknown profile '{}'.",
                action_name, profile_name
            ));
        }
        command.arg("--profile");
        command.arg(profile_name);
    }
    let (child_args, resolution_notes) = child_input_args(
        step.input_overrides.as_deref(),
        step.input_mode,
        step.inputs.as_deref(),
        data,
        action_name,
        named_inputs,
    )?;
    for note in resolution_notes {
        print_action_line(action_index, action_name, note.as_str());
    }
    for argument in child_args {
        command.arg(argument);
    }
    command.env(AGENT_ACTION_DEPTH_ENV, (current_depth + 1).to_string());
    command.env(AGENT_ACTION_MAX_DEPTH_ENV, max_agent_depth.to_string());
    command.env(
        AGENT_ACTION_MAX_RUNTIME_SECS_ENV,
        runtime_budget.max_runtime_secs.to_string(),
    );
    command.env(
        AGENT_ACTION_RUNTIME_STARTED_AT_MS_ENV,
        runtime_budget.started_at_ms.to_string(),
    );
    command.env(
        AGENT_ACTION_RUNTIME_DEADLINE_MS_ENV,
        runtime_budget.deadline_ms.to_string(),
    );
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());

    let remaining = remaining_runtime_duration(
        runtime_budget,
        &format!("before starting child agent '{}'", agent),
    )
    .map_err(|context| {
        action_runtime_timeout_message(action_name, runtime_budget, context.as_str())
    })?;

    let child = command.spawn().map_err(|error| {
        format!(
            "Action '{}' failed to start child agent '{}': {}",
            action_name, agent, error
        )
    })?;
    let mut child = child;
    print_action_line(
        action_index,
        action_name,
        format!("child: started {}", agent).as_str(),
    );

    let result = match tokio::time::timeout(remaining, child.wait()).await {
        Ok(Ok(status)) if status.success() => {
            print_action_line(action_index, action_name, "child: completed successfully");
            Ok(StepExecutionOutcome::Completed)
        }
        Ok(Ok(status)) => {
            print_action_line(
                action_index,
                action_name,
                format!("child: exited with status {}", status).as_str(),
            );
            Err(format!(
                "Action '{}' child agent '{}' exited with status {} at depth {}.",
                action_name,
                agent,
                status,
                current_depth + 1
            ))
        }
        Ok(Err(error)) => Err(format!(
            "Action '{}' failed while waiting for child agent '{}' at depth {}: {}",
            action_name,
            agent,
            current_depth + 1,
            error
        )),
        Err(_) => {
            let _ = child.kill().await;
            print_action_line(
                action_index,
                action_name,
                format!("child: timed out {}", agent).as_str(),
            );
            Err(action_runtime_timeout_message(
                action_name,
                runtime_budget,
                &format!(
                    "while waiting for child agent '{}' at depth {}",
                    agent,
                    current_depth + 1
                ),
            ))
        }
    };
    result
}

fn action_completion_summary(outcomes: &[StepExecutionOutcome]) -> Option<&'static str> {
    if outcomes.is_empty()
        || outcomes.iter().any(|outcome| {
            matches!(
                outcome,
                StepExecutionOutcome::SoftFailureLogged
                    | StepExecutionOutcome::SuccessAlreadyPrinted
            )
        })
    {
        None
    } else {
        Some("completed")
    }
}

fn format_top_level_action_failures(failures: &[String]) -> String {
    if failures.len() == 1 {
        failures
            .first()
            .expect("single failure should exist")
            .clone()
    } else {
        format!(
            "{} top-level actions failed:\n{}",
            failures.len(),
            failures.join("\n")
        )
    }
}

fn format_abort_summary(abort: &InvocationAbortRecord) -> String {
    format!(
        "Run aborted by {}: {}",
        action_lane_prefix(abort.action_index, abort.action_name.as_str()),
        abort.error
    )
}

fn run_completion_message_for_depth(depth: u32, elapsed: Duration) -> Option<String> {
    if depth == 0 {
        Some(format!(
            "✅ Run complete in {}.",
            format_elapsed_duration(elapsed)
        ))
    } else {
        None
    }
}

fn root_run_completion_message(elapsed: Duration) -> Option<String> {
    run_completion_message_for_depth(current_agent_action_depth(), elapsed)
}

fn root_run_failure_message(elapsed: Duration) -> String {
    format!("❌ Run failed in {}.", format_elapsed_duration(elapsed))
}

fn root_run_abort_message(elapsed: Duration) -> String {
    format!("❌ Run aborted in {}.", format_elapsed_duration(elapsed))
}

fn current_action_output() -> Option<ActionOutput> {
    ACTION_OUTPUT.try_with(Clone::clone).ok()
}

fn finish_action_output() {
    if let Some(output) = current_action_output() {
        output.finish();
    }
}

fn note_action_step_started(
    action_index: usize,
    action_name: &str,
    step_kind: &str,
    step_number: usize,
    step_count: usize,
) {
    if let Some(output) = current_action_output() {
        output.action_step_started(
            action_index,
            action_name,
            step_kind,
            step_number,
            step_count,
        );
    }
}

fn note_action_failure(action_index: usize, action_name: &str, error: &str) {
    if let Some(output) = current_action_output() {
        output.action_failed(action_index, action_name, error);
    }
}

fn note_action_abort(action_index: usize, action_name: &str, error: &str) {
    if let Some(output) = current_action_output() {
        output.action_aborted(action_index, action_name, error);
    }
}

fn note_action_stopped_by_abort(action_index: usize, action_name: &str) {
    if let Some(output) = current_action_output() {
        output.action_stopped_by_abort(action_index, action_name);
    }
}

fn action_execution_header(action_execution: crate::ActionExecutionMode) -> &'static str {
    match action_execution {
        crate::ActionExecutionMode::Sequential => "Action execution: sequential",
        crate::ActionExecutionMode::Parallel => "Action execution: parallel",
    }
}

fn print_action_execution_header(action_execution: crate::ActionExecutionMode) {
    if let Some(output) = current_action_output() {
        output.print_execution_header();
    } else {
        println!("{}", action_execution_header(action_execution));
    }
}

fn action_lane_prefix(action_index: usize, action_name: &str) -> String {
    format!("[A{} {}]", action_index + 1, action_name)
}

fn format_action_line(action_index: usize, action_name: &str, message: &str) -> String {
    format!(
        "{} {}",
        action_lane_prefix(action_index, action_name),
        message
    )
}

fn format_action_failure(action_index: usize, action_name: &str, error: &str) -> String {
    format_action_line(
        action_index,
        action_name,
        format!("failed: {}", error).as_str(),
    )
}

fn print_action_line(action_index: usize, action_name: &str, message: &str) {
    if let Some(output) = current_action_output() {
        output.action_line(action_index, action_name, message);
    } else {
        println!("{}", format_action_line(action_index, action_name, message));
    }
}

fn print_action_start(action_index: usize, action_name: &str) {
    if let Some(output) = current_action_output() {
        output.action_started(action_index, action_name);
    } else {
        print_action_line(action_index, action_name, "started");
    }
}

fn print_action_success(action_index: usize, action_name: &str, summary: &str) {
    if let Some(output) = current_action_output() {
        output.action_success(action_index, action_name, summary);
    } else {
        print_action_line(action_index, action_name, format!("{}.", summary).as_str());
    }
}

fn render_backend_ui_or_json_lines(response: &serde_json::Value) -> Vec<String> {
    if let Some(message) = format_backend_ui_message(response, true) {
        split_action_output_lines(message.as_str())
    } else {
        split_action_output_lines(pretty_backend_json(response).as_str())
    }
}

fn format_backend_error_message(response: &serde_json::Value) -> Option<String> {
    format_backend_ui_message(response, false)
}

fn format_backend_ui_message(
    response: &serde_json::Value,
    include_kind_prefix: bool,
) -> Option<String> {
    let ui = response.get("ui")?;
    if ui.get("schema").and_then(|value| value.as_str()) != Some("1.0") {
        return None;
    }

    let kind = ui
        .get("kind")
        .and_then(|value| value.as_str())
        .unwrap_or("info");
    let title = ui
        .get("title")
        .and_then(|value| value.as_str())
        .unwrap_or("Status");
    let summary = ui
        .get("summary")
        .and_then(|value| value.as_str())
        .unwrap_or("Status response received.");

    let mut lines = Vec::new();
    if include_kind_prefix {
        let kind_prefix = match kind {
            "success" => "✅",
            "error" => "⚠️",
            "failure" => "❌",
            _ => "ℹ️",
        };
        lines.push(format!("{kind_prefix} {title}"));
    } else {
        lines.push(title.to_string());
    }
    lines.push(summary.to_string());

    if let Some(variant) = ui.get("variant").and_then(|value| value.as_str()) {
        if !variant.trim().is_empty() {
            lines.push(format!("Variant: {variant}"));
        }
    }

    if let Some(sections) = ui.get("sections").and_then(|value| value.as_array()) {
        for section in sections {
            append_backend_section_lines(section, &mut lines);
        }
    }

    if let Some(actions) = ui.get("actions").and_then(|value| value.as_array()) {
        let action_lines = actions
            .iter()
            .filter_map(|action| {
                let label = action
                    .get("label")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let command = action
                    .get("command")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");

                if label.is_empty() && command.is_empty() {
                    None
                } else if !label.is_empty() && !command.is_empty() {
                    Some(format!("- {}: {}", label, command))
                } else if !label.is_empty() {
                    Some(format!("- {}", label))
                } else {
                    Some(format!("- {}", command))
                }
            })
            .collect::<Vec<_>>();

        if !action_lines.is_empty() {
            lines.push(String::new());
            lines.push("Actions:".to_string());
            lines.extend(action_lines);
        }
    }

    if let Some(next_steps) = ui.get("next_steps").and_then(|value| value.as_array()) {
        let step_lines = next_steps
            .iter()
            .filter_map(|step| step.as_str())
            .map(str::trim)
            .filter(|step| !step.is_empty())
            .map(|step| format!("- {}", step))
            .collect::<Vec<_>>();

        if !step_lines.is_empty() {
            lines.push(String::new());
            lines.push("Next steps:".to_string());
            lines.extend(step_lines);
        }
    }

    Some(lines.join("\n"))
}

fn append_backend_section_lines(section: &serde_json::Value, lines: &mut Vec<String>) {
    let section_type = section
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let title = section
        .get("title")
        .and_then(|value| value.as_str())
        .unwrap_or("");

    if !title.is_empty() {
        lines.push(String::new());
        lines.push(format!("{title}:"));
    }

    match section_type {
        "kv" => {
            if let Some(items) = section.get("items").and_then(|value| value.as_array()) {
                for item in items {
                    let label = item
                        .get("label")
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    let value = item
                        .get("value")
                        .map(backend_ui_value_to_string)
                        .unwrap_or_default();

                    if label.is_empty() && value.is_empty() {
                        continue;
                    }

                    if label.is_empty() {
                        lines.push(format!("- {}", value));
                    } else {
                        lines.push(format!("- {}: {}", label, value));
                    }
                }
            }
        }
        "list" => {
            if let Some(items) = section.get("items").and_then(|value| value.as_array()) {
                for item in items {
                    let value = backend_ui_value_to_string(item);
                    if !value.is_empty() {
                        lines.push(format!("- {}", value));
                    }
                }
            }
        }
        "notice" => {
            if let Some(message) = section.get("message").and_then(|value| value.as_str()) {
                if !message.trim().is_empty() {
                    lines.push(message.to_string());
                }
            }
        }
        "json" => {
            if let Some(data) = section.get("data") {
                match serde_json::to_string_pretty(data) {
                    Ok(pretty) => lines.extend(pretty.lines().map(str::to_string)),
                    Err(_) => lines.push(backend_ui_value_to_string(data)),
                }
            }
        }
        _ => {}
    }
}

fn backend_ui_value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(boolean) => boolean.to_string(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text.to_string(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            serde_json::to_string(value).unwrap_or_default()
        }
    }
}

fn pretty_backend_json(response: &serde_json::Value) -> String {
    serde_json::to_string_pretty(response).unwrap_or_else(|_| format!("{response:?}"))
}

fn load_account_auth() -> Result<AccountAuth, String> {
    let cfg = load_config().ok_or_else(|| {
        format!(
            "❌ No local config file found at '{}'. Run `cargo ai account register <email>` on this machine, or copy your config from another machine.",
            config_path().display()
        )
    })?;

    let acct = cfg.account.as_ref().ok_or_else(|| {
        "❌ No account found in config. You must confirm your account first.".to_string()
    })?;

    if let Some(account_tokens) = store::load_account_tokens()
        .map_err(|error| format!("❌ Failed to load account credentials: {error}"))?
    {
        return Ok(AccountAuth {
            access_token: account_tokens.access_token,
            refresh_token: account_tokens.refresh_token,
        });
    }

    let access_token = acct.access_token.as_ref().cloned().ok_or_else(|| {
        "❌ No access token found in credentials store or legacy config. Run `cargo ai account confirm <code>` first."
            .to_string()
    })?;

    Ok(AccountAuth {
        access_token,
        refresh_token: acct.refresh_token.clone(),
    })
}

async fn refresh_access_token_for_retry(
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<(String, Option<i32>), RefreshAccessError> {
    let rt = refresh_token.ok_or(RefreshAccessError::MissingRefreshToken)?;

    let refresh_response =
        infra_api::account::status::fetch_status(INFRA_BASE_URL, access_token, Some(rt))
            .await
            .map_err(|error| RefreshAccessError::RequestFailed(format!("{error:?}")))?;

    let refreshed_access_token = refresh_response
        .get("session")
        .and_then(|session| session.get("access_token"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    let refreshed_expires_in = refresh_response
        .get("session")
        .and_then(|session| session.get("expires_in_seconds"))
        .and_then(|value| value.as_i64())
        .and_then(|value| i32::try_from(value).ok());

    match refreshed_access_token {
        Some(token) => Ok((token, refreshed_expires_in)),
        None => Err(RefreshAccessError::MissingRefreshedToken(refresh_response)),
    }
}

fn persist_refreshed_access_token(
    refreshed_access_token: &str,
    refresh_token: &str,
    refreshed_expires_in: Option<i32>,
) {
    if let Some(expires_in) = refreshed_expires_in {
        if let Err(error) = set_account_tokens(
            refreshed_access_token.to_string(),
            refresh_token.to_string(),
            expires_in,
        ) {
            eprintln!("⚠️ Failed to update account tokens in credential store: {error}");
        }
    }
}

fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn new_runtime_budget(max_runtime_secs: u64) -> InvocationRuntimeBudget {
    let started_at_ms = current_time_millis();
    InvocationRuntimeBudget {
        max_runtime_secs,
        started_at_ms,
        deadline_ms: started_at_ms.saturating_add(max_runtime_secs.saturating_mul(1000)),
    }
}

fn inherited_agent_action_runtime_budget() -> Option<InvocationRuntimeBudget> {
    let max_runtime_secs = std::env::var(AGENT_ACTION_MAX_RUNTIME_SECS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())?;
    let started_at_ms = std::env::var(AGENT_ACTION_RUNTIME_STARTED_AT_MS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())?;
    let deadline_ms = std::env::var(AGENT_ACTION_RUNTIME_DEADLINE_MS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())?;

    Some(InvocationRuntimeBudget {
        max_runtime_secs,
        started_at_ms,
        deadline_ms,
    })
}

pub(crate) fn configured_agent_action_runtime_budget(
    cli_override: Option<u64>,
) -> InvocationRuntimeBudget {
    cli_override
        .map(new_runtime_budget)
        .or_else(inherited_agent_action_runtime_budget)
        .unwrap_or_else(|| new_runtime_budget(DEFAULT_AGENT_ACTION_MAX_RUNTIME_SECS))
}

fn remaining_runtime_duration(
    runtime_budget: InvocationRuntimeBudget,
    exhausted_context: &str,
) -> Result<Duration, String> {
    let now = current_time_millis();
    if now >= runtime_budget.deadline_ms {
        return Err(exhausted_context.to_string());
    }

    Ok(Duration::from_millis(
        runtime_budget.deadline_ms.saturating_sub(now),
    ))
}

fn elapsed_runtime_secs(runtime_budget: InvocationRuntimeBudget) -> u64 {
    current_time_millis()
        .saturating_sub(runtime_budget.started_at_ms)
        .div_ceil(1000)
}

fn action_runtime_timeout_message(
    action_name: &str,
    runtime_budget: InvocationRuntimeBudget,
    context: &str,
) -> String {
    format!(
        "Action '{}' exceeded max-runtime-in-sec {} after {} seconds {}.",
        action_name,
        runtime_budget.max_runtime_secs,
        elapsed_runtime_secs(runtime_budget),
        context
    )
}

fn validate_agent_step_target(agent: &str, action_name: &str) -> Result<(), String> {
    let agent_path = Path::new(agent);
    if agent.trim().is_empty() {
        return Err(format!(
            "Action '{}' agent step target '{}' must use explicit same-level './childagent' form.",
            action_name, agent
        ));
    }

    if agent_path.is_absolute() {
        return Err(format!(
            "Action '{}' agent step target '{}' must use explicit same-level './childagent' form; absolute paths are not allowed.",
            action_name, agent
        ));
    }

    if agent_path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "Action '{}' agent step target '{}' must use explicit same-level './childagent' form; parent traversal (`..`) is not allowed.",
            action_name, agent
        ));
    }

    if !agent.starts_with("./") {
        let message = if contains_explicit_path_separator(agent) {
            "must use explicit same-level './childagent' form; nested child-agent paths are not allowed."
        } else {
            "must use explicit same-level './childagent' form; bare child-agent names are not allowed."
        };
        return Err(format!(
            "Action '{}' agent step target '{}' {}",
            action_name, agent, message
        ));
    }

    let sibling = &agent[2..];
    if sibling.is_empty() || !is_single_normal_path_component(sibling) {
        return Err(format!(
            "Action '{}' agent step target '{}' must stay at the same level; nested child-agent paths such as './agents/childagent' are not allowed.",
            action_name, agent
        ));
    }

    Ok(())
}

fn contains_explicit_path_separator(path: &str) -> bool {
    path.contains('/') || path.contains('\\')
}

fn is_single_normal_path_component(path: &str) -> bool {
    let mut components = Path::new(path).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn validate_agent_action_depth(
    current_depth: u32,
    max_agent_depth: u32,
    action_name: &str,
) -> Result<(), String> {
    if current_depth >= max_agent_depth {
        return Err(format!(
            "Action '{}' cannot invoke another agent because current depth {} has reached max-agent-depth {}.",
            action_name, current_depth, max_agent_depth
        ));
    }

    Ok(())
}

fn current_action_platform() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("macos")
    } else if cfg!(target_os = "linux") {
        Some("linux")
    } else if cfg!(target_os = "windows") {
        Some("windows")
    } else {
        None
    }
}

fn matching_run_steps<'a>(
    run_steps: &'a [crate::RunStep],
    current_platform: Option<&str>,
) -> Vec<&'a crate::RunStep> {
    run_steps
        .iter()
        .filter(|step| step_matches_platform(step.platforms.as_deref(), current_platform))
        .collect()
}

fn child_input_args(
    input_overrides: Option<&[crate::ActionInputOverride]>,
    input_mode: Option<crate::ActionInputMode>,
    inputs: Option<&[crate::ActionInput]>,
    data: &serde_json::Value,
    action_name: &str,
    named_inputs: &BTreeMap<String, crate::Input>,
) -> Result<(Vec<String>, Vec<String>), String> {
    let mut args = Vec::new();
    let mut notes = Vec::new();

    if let Some(input_overrides) = input_overrides {
        for input_override in input_overrides {
            let (resolved_value, resolution_note) = resolve_child_input_override_value(
                &input_override.value,
                data,
                action_name,
                &input_override.name,
                named_inputs,
            )?;
            args.push("--input-override".to_string());
            args.push(format!("{}={}", input_override.name, resolved_value));
            if let Some(note) = resolution_note {
                notes.push(note);
            }
        }
    }

    if let Some(input_mode) = input_mode {
        if inputs.is_none() {
            return Err(format!(
                "Action '{}' child-agent `input_mode` requires `inputs`.",
                action_name
            ));
        }

        args.push("--input-mode".to_string());
        args.push(
            match input_mode {
                crate::ActionInputMode::Replace => "replace",
                crate::ActionInputMode::Append => "append",
                crate::ActionInputMode::Prepend => "prepend",
            }
            .to_string(),
        );
    }

    if let Some(inputs) = inputs {
        for (index, input) in inputs.iter().enumerate() {
            match input {
                crate::ActionInput::Text { text } => {
                    let resolved = resolve_string_parts(
                        text,
                        data,
                        action_name,
                        &format!("child-agent text input {}", index + 1),
                    )?;
                    args.push("--input-text".to_string());
                    args.push(resolved);
                    if child_input_uses_dynamic_parts(text) {
                        notes.push(format!(
                            "Action '{}' resolved dynamic child-agent text input {}.",
                            action_name,
                            index + 1
                        ));
                    }
                }
                crate::ActionInput::Url { url } => {
                    let resolved = resolve_string_parts(
                        url,
                        data,
                        action_name,
                        &format!("child-agent url input {}", index + 1),
                    )?;
                    validate_child_input_url(&resolved, action_name, index + 1)?;
                    args.push("--input-url".to_string());
                    args.push(resolved.clone());
                    if child_input_uses_dynamic_parts(url) {
                        notes.push(format!(
                            "Action '{}' resolved dynamic child-agent url input {} -> {}.",
                            action_name,
                            index + 1,
                            resolved
                        ));
                    }
                }
                crate::ActionInput::Image { path } => {
                    let resolved = resolve_string_parts(
                        path,
                        data,
                        action_name,
                        &format!("child-agent image path input {}", index + 1),
                    )?;
                    validate_child_input_path(&resolved, action_name, index + 1, "image")?;
                    args.push("--input-image".to_string());
                    args.push(resolved.clone());
                    if child_input_uses_dynamic_parts(path) {
                        notes.push(format!(
                            "Action '{}' resolved dynamic child-agent image path input {} -> {}.",
                            action_name,
                            index + 1,
                            resolved
                        ));
                    }
                }
                crate::ActionInput::File { path } => {
                    let resolved = resolve_string_parts(
                        path,
                        data,
                        action_name,
                        &format!("child-agent file path input {}", index + 1),
                    )?;
                    validate_child_input_path(&resolved, action_name, index + 1, "file")?;
                    validate_child_file_extension(&resolved, action_name, index + 1)?;
                    args.push("--input-file".to_string());
                    args.push(resolved.clone());
                    if child_input_uses_dynamic_parts(path) {
                        notes.push(format!(
                            "Action '{}' resolved dynamic child-agent file path input {} -> {}.",
                            action_name,
                            index + 1,
                            resolved
                        ));
                    }
                }
                crate::ActionInput::Named { input } => {
                    let forwarded = named_inputs.get(input).ok_or_else(|| {
                        format!(
                            "Action '{}' child-agent named input '{}' is not available for forwarding.",
                            action_name, input
                        )
                    })?;
                    let value = forwarded.value.as_deref().ok_or_else(|| {
                        format!(
                            "Action '{}' child-agent named input '{}' is required but unresolved for this invocation.",
                            action_name, input
                        )
                    })?;
                    let payload = crate::Input {
                        name: Some(input.clone()),
                        kind: forwarded.kind,
                        value: Some(value.to_string()),
                    };
                    args.push("--forwarded-input".to_string());
                    args.push(serde_json::to_string(&payload).map_err(|error| {
                        format!(
                            "Action '{}' failed to serialize forwarded named input '{}': {}",
                            action_name, input, error
                        )
                    })?);
                }
            }
        }
    }

    Ok((args, notes))
}

fn resolve_child_input_override_value(
    input: &crate::ActionInput,
    data: &serde_json::Value,
    action_name: &str,
    override_name: &str,
    named_inputs: &BTreeMap<String, crate::Input>,
) -> Result<(String, Option<String>), String> {
    match input {
        crate::ActionInput::Text { text } => {
            let resolved = resolve_string_parts(
                text,
                data,
                action_name,
                &format!("child-agent named input override '{}'", override_name),
            )?;
            let note = child_input_uses_dynamic_parts(text).then(|| {
                format!(
                    "Action '{}' resolved dynamic child-agent named text override '{}'.",
                    action_name, override_name
                )
            });
            Ok((resolved, note))
        }
        crate::ActionInput::Url { url } => {
            let resolved = resolve_string_parts(
                url,
                data,
                action_name,
                &format!("child-agent named url override '{}'", override_name),
            )?;
            validate_child_input_override_url(&resolved, action_name, override_name)?;
            let note = child_input_uses_dynamic_parts(url).then(|| {
                format!(
                    "Action '{}' resolved dynamic child-agent named url override '{}' -> {}.",
                    action_name, override_name, resolved
                )
            });
            Ok((resolved, note))
        }
        crate::ActionInput::Image { path } => {
            let resolved = resolve_string_parts(
                path,
                data,
                action_name,
                &format!("child-agent named image override '{}'", override_name),
            )?;
            let note = child_input_uses_dynamic_parts(path).then(|| {
                format!(
                    "Action '{}' resolved dynamic child-agent named image override '{}' -> {}.",
                    action_name, override_name, resolved
                )
            });
            Ok((resolved, note))
        }
        crate::ActionInput::File { path } => {
            let resolved = resolve_string_parts(
                path,
                data,
                action_name,
                &format!("child-agent named file override '{}'", override_name),
            )?;
            validate_child_input_override_file_extension(&resolved, action_name, override_name)?;
            let note = child_input_uses_dynamic_parts(path).then(|| {
                format!(
                    "Action '{}' resolved dynamic child-agent named file override '{}' -> {}.",
                    action_name, override_name, resolved
                )
            });
            Ok((resolved, note))
        }
        crate::ActionInput::Named { input } => {
            let forwarded = named_inputs.get(input).ok_or_else(|| {
                format!(
                    "Action '{}' child-agent named input '{}' is not available for forwarding.",
                    action_name, input
                )
            })?;
            let value = forwarded.value.as_deref().ok_or_else(|| {
                format!(
                    "Action '{}' child-agent named input '{}' is required but unresolved for this invocation.",
                    action_name, input
                )
            })?;
            Ok((value.to_string(), None))
        }
    }
}

fn child_input_uses_dynamic_parts(parts: &[crate::RunArg]) -> bool {
    parts
        .iter()
        .any(|part| matches!(part, crate::RunArg::Variable(_)))
}

fn validate_generated_image_output_path(path: &Path, action_name: &str) -> Result<(), String> {
    let raw_path = path.to_string_lossy();
    if raw_path.trim().is_empty() {
        return Err(format!(
            "Action '{}' generate_image `path` must resolve to a non-empty relative path.",
            action_name
        ));
    }
    if path.is_absolute() {
        return Err(format!(
            "Action '{}' generate_image `path` must resolve to a relative path.",
            action_name
        ));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "Action '{}' generate_image `path` must not use parent traversal (`..`).",
            action_name
        ));
    }

    generated_image_output_format(raw_path.as_ref(), action_name).map(|_| ())
}

fn generated_image_output_format(
    raw_path: &str,
    action_name: &str,
) -> Result<&'static str, String> {
    let extension = Path::new(raw_path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());

    match extension.as_deref() {
        Some("png") => Ok("png"),
        Some("jpg") | Some("jpeg") => Ok("jpeg"),
        Some("webp") => Ok("webp"),
        _ => Err(format!(
            "Action '{}' generate_image `path` must use a supported extension: `.png`, `.jpg`, `.jpeg`, `.webp`.",
            action_name
        )),
    }
}

fn validate_child_input_url(
    url: &str,
    action_name: &str,
    input_index: usize,
) -> Result<(), String> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        Err(format!(
            "Action '{}' child-agent url input {} must resolve to an http(s) URL.",
            action_name, input_index
        ))
    }
}

fn validate_child_input_override_url(
    url: &str,
    action_name: &str,
    override_name: &str,
) -> Result<(), String> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        Err(format!(
            "Action '{}' child-agent named input override '{}' must resolve to an absolute http(s) URL.",
            action_name, override_name
        ))
    }
}

fn validate_child_input_path(
    path: &str,
    action_name: &str,
    input_index: usize,
    input_kind: &str,
) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err(format!(
            "Action '{}' child-agent {} input {} must resolve to a non-empty relative path.",
            action_name, input_kind, input_index
        ));
    }

    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return Err(format!(
            "Action '{}' child-agent {} input {} must stay at the current level or below; absolute paths are not allowed.",
            action_name, input_kind, input_index
        ));
    }

    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "Action '{}' child-agent {} input {} must stay at the current level or below; parent traversal (`..`) is not allowed.",
            action_name, input_kind, input_index
        ));
    }

    Ok(())
}

fn validate_child_input_override_file_extension(
    path: &str,
    action_name: &str,
    override_name: &str,
) -> Result<(), String> {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());

    match extension.as_deref() {
        Some(
            "pdf" | "docx" | "csv" | "xla" | "xlb" | "xlc" | "xlm" | "xls" | "xlsx" | "xlt"
                | "xlw" | "tsv" | "iif" | "doc" | "dot" | "odt" | "rtf" | "pot" | "ppa"
                | "pps" | "ppt" | "pptx" | "pwz" | "wiz",
        ) => Ok(()),
        _ => Err(format!(
            "Action '{}' child-agent named input override '{}' must use a supported file extension: {}.",
            action_name, override_name, SUPPORTED_FILE_EXTENSIONS_MESSAGE
        )),
    }
}

fn validate_child_file_extension(
    path: &str,
    action_name: &str,
    input_index: usize,
) -> Result<(), String> {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());

    match extension.as_deref() {
        Some(
            "pdf" | "docx" | "csv" | "xla" | "xlb" | "xlc" | "xlm" | "xls" | "xlsx" | "xlt" | "xlw"
            | "tsv" | "iif" | "doc" | "dot" | "odt" | "rtf" | "pot" | "ppa" | "pps" | "ppt"
            | "pptx" | "pwz" | "wiz",
        ) => Ok(()),
        _ => Err(format!(
            "Action '{}' child-agent file input {} must use a supported extension: {}.",
            action_name, input_index, SUPPORTED_FILE_EXTENSIONS_MESSAGE
        )),
    }
}

fn current_agent_action_depth() -> u32 {
    std::env::var(AGENT_ACTION_DEPTH_ENV)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0)
}

fn step_matches_platform(platforms: Option<&[String]>, current_platform: Option<&str>) -> bool {
    match platforms {
        None => true,
        Some(platforms) => current_platform
            .is_some_and(|platform| platforms.iter().any(|candidate| candidate == platform)),
    }
}

fn resolve_run_args(
    args: &[crate::RunArg],
    data: &serde_json::Value,
    action_name: &str,
) -> Result<Vec<String>, String> {
    args.iter()
        .enumerate()
        .map(|(index, arg)| resolve_run_arg(arg, data, action_name, index))
        .collect()
}

fn resolve_string_parts(
    parts: &[crate::RunArg],
    data: &serde_json::Value,
    action_name: &str,
    field_name: &str,
) -> Result<String, String> {
    let mut resolved = String::new();

    for (index, part) in parts.iter().enumerate() {
        let value = resolve_run_arg(part, data, action_name, index)?;
        resolved.push_str(&value);
    }

    if resolved.trim().is_empty() {
        return Err(format!(
            "Action '{}' {} resolved to an empty string.",
            action_name, field_name
        ));
    }

    Ok(resolved)
}

fn resolve_run_arg(
    arg: &crate::RunArg,
    data: &serde_json::Value,
    action_name: &str,
    index: usize,
) -> Result<String, String> {
    match arg {
        crate::RunArg::Literal(literal) => Ok(literal.clone()),
        crate::RunArg::Variable(variable) => {
            let Some(value) = lookup_action_variable(data, variable) else {
                return Err(format!(
                    "Action '{}' arg {} references missing variable '{}'.",
                    action_name, index, variable
                ));
            };

            match value {
                serde_json::Value::String(text) => Ok(text.clone()),
                serde_json::Value::Bool(boolean) => Ok(boolean.to_string()),
                serde_json::Value::Number(number) => Ok(number.to_string()),
                serde_json::Value::Array(_) => Err(format!(
                    "Action '{}' arg {} references array-valued variable '{}', which is unsupported for arg substitution.",
                    action_name, index, variable
                )),
                serde_json::Value::Object(_) => Err(format!(
                    "Action '{}' arg {} references object-valued variable '{}', which is unsupported for arg substitution.",
                    action_name, index, variable
                )),
                serde_json::Value::Null => Err(format!(
                    "Action '{}' arg {} references null variable '{}', which is unsupported for arg substitution.",
                    action_name, index, variable
                )),
            }
        }
    }
}

fn lookup_action_variable<'a>(
    data: &'a serde_json::Value,
    variable: &str,
) -> Option<&'a serde_json::Value> {
    if let Some(runtime_name) = variable.strip_prefix("runtime.") {
        return data
            .get("runtime")
            .and_then(serde_json::Value::as_object)
            .and_then(|runtime| runtime.get(runtime_name));
    }

    if variable.contains('.') {
        return None;
    }

    data.get(variable)
}

#[cfg(test)]
mod tests {
    use super::{
        action_completion_summary, action_execution_header, action_lane_prefix, apply_actions,
        child_input_args, configured_agent_action_runtime_budget, format_backend_error_message,
        format_backend_ui_message, insert_action_output_variable, matching_run_steps,
        resolve_run_args, resolve_string_parts, run_agent_step, run_completion_message_for_depth,
        run_exec_step, run_generate_image_step, step_matches_platform, validate_agent_action_depth,
        ActionOutput, ActionOutputMode, ActionProviderContext, StepExecutionOutcome, ACTION_OUTPUT,
    };
    use crate::providers::ProviderKind;
    use serde_json::json;
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard};
    use std::time::{SystemTime, UNIX_EPOCH};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct TestCargoHome {
        _guard: MutexGuard<'static, ()>,
        original_cargo_home: Option<OsString>,
        original_disable_keychain: Option<OsString>,
        root: PathBuf,
    }

    impl TestCargoHome {
        fn new(config_toml: &str) -> Self {
            let guard = ENV_LOCK
                .lock()
                .expect("environment lock should not be poisoned");
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be valid")
                .as_nanos();
            let root = std::env::temp_dir().join(format!("cargo-ai-preflight-actions-{unique}"));
            let config_path = root.join(".cargo-ai").join("config.toml");
            fs::create_dir_all(
                config_path
                    .parent()
                    .expect("config path should have a parent directory"),
            )
            .expect("temp config dir should be created");
            fs::write(&config_path, config_toml).expect("temp config should be written");

            let original_cargo_home = std::env::var_os("CARGO_HOME");
            let original_disable_keychain = std::env::var_os("CARGO_AI_DISABLE_KEYCHAIN");
            std::env::set_var("CARGO_HOME", &root);
            std::env::set_var("CARGO_AI_DISABLE_KEYCHAIN", "1");

            Self {
                _guard: guard,
                original_cargo_home,
                original_disable_keychain,
                root,
            }
        }
    }

    impl Drop for TestCargoHome {
        fn drop(&mut self) {
            match &self.original_cargo_home {
                Some(value) => std::env::set_var("CARGO_HOME", value),
                None => std::env::remove_var("CARGO_HOME"),
            }
            match &self.original_disable_keychain {
                Some(value) => std::env::set_var("CARGO_AI_DISABLE_KEYCHAIN", value),
                None => std::env::remove_var("CARGO_AI_DISABLE_KEYCHAIN"),
            }

            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn run_step(
        program: &str,
        platforms: Option<&[&str]>,
        args: Vec<crate::RunArg>,
    ) -> crate::RunStep {
        crate::RunStep {
            kind: "exec".to_string(),
            program: Some(program.to_string()),
            model: None,
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args,
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: None,
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: platforms.map(|platforms| {
                platforms
                    .iter()
                    .map(|platform| platform.to_string())
                    .collect()
            }),
        }
    }

    fn provider_context() -> ActionProviderContext {
        ActionProviderContext {
            provider: ProviderKind::OpenAi,
            model: "gpt-5.2".to_string(),
            url: "https://api.openai.com/v1/chat/completions".to_string(),
            token: "test-token".to_string(),
            inference_timeout_in_sec: 60,
        }
    }

    fn profile_config(profile_name: &str, server_url: &str, model: &str) -> String {
        format!(
            r#"
[[profile]]
name = "{profile_name}"
server = "openai"
model = "{model}"
url = "{server_url}"
token = "profile-token"
timeout_in_sec = 42
auth_mode = "api_key"
"#
        )
    }

    fn action(run: Vec<crate::RunStep>) -> crate::Action {
        crate::Action {
            name: "demo".to_string(),
            logic: json!({ "==": [{ "var": "answer" }, 4] }),
            run,
        }
    }

    fn runtime_vars(
        entries: &[(&str, serde_json::Value)],
    ) -> serde_json::Map<String, serde_json::Value> {
        entries
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect()
    }

    fn no_named_inputs() -> std::collections::BTreeMap<String, crate::Input> {
        std::collections::BTreeMap::new()
    }

    fn named_input(name: &str, kind: crate::InputKind, value: Option<&str>) -> crate::Input {
        crate::Input {
            name: Some(name.to_string()),
            kind,
            value: value.map(str::to_string),
        }
    }

    #[test]
    fn platformless_steps_match_supported_platforms() {
        assert!(step_matches_platform(None, Some("macos")));
        assert!(step_matches_platform(None, Some("linux")));
        assert!(step_matches_platform(None, None));
    }

    #[test]
    fn explicit_platforms_match_only_listed_platforms() {
        let platforms = vec!["macos".to_string(), "linux".to_string()];
        assert!(step_matches_platform(Some(&platforms), Some("macos")));
        assert!(step_matches_platform(Some(&platforms), Some("linux")));
        assert!(!step_matches_platform(Some(&platforms), Some("windows")));
        assert!(!step_matches_platform(Some(&platforms), None));
    }

    #[test]
    fn matching_run_steps_preserve_declared_order() {
        let run_steps = vec![
            run_step("first", Some(&["windows"]), vec![]),
            run_step("second", None, vec![]),
            run_step("third", Some(&["macos", "linux"]), vec![]),
            run_step("fourth", None, vec![]),
        ];

        let matching = matching_run_steps(&run_steps, Some("macos"));
        let programs = matching
            .iter()
            .map(|step| {
                step.program
                    .as_deref()
                    .expect("exec test steps have a program")
            })
            .collect::<Vec<_>>();

        assert_eq!(programs, vec!["second", "third", "fourth"]);
    }

    #[test]
    fn resolves_literal_and_variable_args() {
        let resolved = resolve_run_args(
            &[
                crate::RunArg::Literal("value=".to_string()),
                crate::RunArg::Variable("answer".to_string()),
                crate::RunArg::Variable("raining".to_string()),
            ],
            &json!({
                "answer": 4,
                "raining": true
            }),
            "demo",
        )
        .expect("args should resolve");

        assert_eq!(resolved, vec!["value=", "4", "true"]);
    }

    #[test]
    fn rejects_missing_variable_args() {
        let error = resolve_run_args(
            &[crate::RunArg::Variable("answer".to_string())],
            &json!({}),
            "demo",
        )
        .unwrap_err();

        assert!(error.contains("missing variable 'answer'"));
    }

    #[test]
    fn rejects_array_valued_variable_args() {
        let error = resolve_run_args(
            &[crate::RunArg::Variable("numbers".to_string())],
            &json!({
                "numbers": [1, 2, 3]
            }),
            "demo",
        )
        .unwrap_err();

        assert!(error.contains("array-valued variable 'numbers'"));
    }

    #[test]
    fn resolves_runtime_variable_args() {
        let resolved = resolve_run_args(
            &[
                crate::RunArg::Literal("mode=".to_string()),
                crate::RunArg::Variable("runtime.image_model".to_string()),
                crate::RunArg::Variable("runtime.generate_images".to_string()),
            ],
            &json!({
                "runtime": {
                    "image_model": "gpt-image-1",
                    "generate_images": true
                }
            }),
            "demo",
        )
        .expect("runtime args should resolve");

        assert_eq!(resolved, vec!["mode=", "gpt-image-1", "true"]);
    }

    #[test]
    fn resolves_runtime_variable_string_parts() {
        let resolved = resolve_string_parts(
            &[
                crate::RunArg::Literal("subject=".to_string()),
                crate::RunArg::Variable("runtime.report_suffix".to_string()),
            ],
            &json!({
                "runtime": {
                    "report_suffix": "nightly"
                }
            }),
            "demo",
            "subject",
        )
        .expect("runtime string parts should resolve");

        assert_eq!(resolved, "subject=nightly");
    }

    #[test]
    fn resolves_string_parts_without_implicit_spaces() {
        let resolved = resolve_string_parts(
            &[
                crate::RunArg::Literal("raining=".to_string()),
                crate::RunArg::Variable("raining".to_string()),
            ],
            &json!({
                "raining": true
            }),
            "demo",
            "text",
        )
        .expect("string parts should resolve");

        assert_eq!(resolved, "raining=true");
    }

    #[test]
    fn child_input_args_map_to_runtime_flags() {
        let (args, notes) = child_input_args(
            None,
            None,
            Some(&[
                crate::ActionInput::Text {
                    text: vec![crate::RunArg::Literal("hello".to_string())],
                },
                crate::ActionInput::Url {
                    url: vec![crate::RunArg::Literal("https://example.com".to_string())],
                },
                crate::ActionInput::Image {
                    path: vec![crate::RunArg::Literal("./diagram.png".to_string())],
                },
                crate::ActionInput::File {
                    path: vec![crate::RunArg::Literal("./report.pdf".to_string())],
                },
            ]),
            &json!({}),
            "demo",
            &no_named_inputs(),
        )
        .expect("child input args should resolve");

        assert_eq!(
            args,
            vec![
                "--input-text",
                "hello",
                "--input-url",
                "https://example.com",
                "--input-image",
                "./diagram.png",
                "--input-file",
                "./report.pdf",
            ]
        );
        assert!(notes.is_empty());
    }

    #[test]
    fn child_input_args_include_explicit_input_mode() {
        let (args, notes) = child_input_args(
            None,
            Some(crate::ActionInputMode::Prepend),
            Some(&[crate::ActionInput::Text {
                text: vec![crate::RunArg::Literal("hello".to_string())],
            }]),
            &json!({}),
            "demo",
            &no_named_inputs(),
        )
        .expect("child input args should resolve");

        assert_eq!(
            args,
            vec!["--input-mode", "prepend", "--input-text", "hello"]
        );
        assert!(notes.is_empty());
    }

    #[test]
    fn child_input_args_reject_input_mode_without_inputs() {
        let error = child_input_args(
            None,
            Some(crate::ActionInputMode::Append),
            None,
            &json!({}),
            "demo",
            &no_named_inputs(),
        )
        .unwrap_err();

        assert!(error.contains("child-agent `input_mode` requires `inputs`"));
    }

    #[test]
    fn child_input_args_resolve_dynamic_text_and_file_path() {
        let (args, notes) = child_input_args(
            None,
            None,
            Some(&[
                crate::ActionInput::Text {
                    text: vec![
                        crate::RunArg::Literal("hello ".to_string()),
                        crate::RunArg::Variable("customer".to_string()),
                    ],
                },
                crate::ActionInput::File {
                    path: vec![
                        crate::RunArg::Literal("./reports/".to_string()),
                        crate::RunArg::Variable("report_filename".to_string()),
                    ],
                },
            ]),
            &json!({
                "customer": "Acme",
                "report_filename": "q1.pdf"
            }),
            "demo",
            &no_named_inputs(),
        )
        .expect("dynamic child input args should resolve");

        assert_eq!(
            args,
            vec![
                "--input-text",
                "hello Acme",
                "--input-file",
                "./reports/q1.pdf"
            ]
        );
        assert_eq!(notes.len(), 2);
        assert!(notes[0].contains("dynamic child-agent text input 1"));
        assert!(notes[1].contains("./reports/q1.pdf"));
    }

    #[test]
    fn child_input_args_reject_invalid_dynamic_url() {
        let error = child_input_args(
            None,
            None,
            Some(&[crate::ActionInput::Url {
                url: vec![crate::RunArg::Variable("source_url".to_string())],
            }]),
            &json!({
                "source_url": "ftp://example.com/report"
            }),
            "demo",
            &no_named_inputs(),
        )
        .unwrap_err();

        assert!(error.contains("must resolve to an http(s) URL"));
    }

    #[test]
    fn child_input_args_reject_invalid_dynamic_file_extension() {
        let error = child_input_args(
            None,
            None,
            Some(&[crate::ActionInput::File {
                path: vec![
                    crate::RunArg::Literal("./reports/".to_string()),
                    crate::RunArg::Variable("report_filename".to_string()),
                ],
            }]),
            &json!({
                "report_filename": "q1.exe"
            }),
            "demo",
            &no_named_inputs(),
        )
        .unwrap_err();

        assert!(error.contains("supported extension"));
    }

    #[test]
    fn child_input_args_reject_parent_traversal_in_dynamic_path() {
        let error = child_input_args(
            None,
            None,
            Some(&[crate::ActionInput::Image {
                path: vec![crate::RunArg::Variable("image_path".to_string())],
            }]),
            &json!({
                "image_path": "../diagram.png"
            }),
            "demo",
            &no_named_inputs(),
        )
        .unwrap_err();

        assert!(error.contains("parent traversal"));
    }

    #[test]
    fn child_input_args_forward_named_inputs_as_hidden_runtime_payloads() {
        let (args, notes) = child_input_args(
            None,
            None,
            Some(&[crate::ActionInput::Named {
                input: "menu_image".to_string(),
            }]),
            &json!({}),
            "demo",
            &std::collections::BTreeMap::from([(
                "menu_image".to_string(),
                named_input(
                    "menu_image",
                    crate::InputKind::Image,
                    Some("./artifacts/menu.png"),
                ),
            )]),
        )
        .expect("named child input args should resolve");

        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "--forwarded-input");
        let payload: crate::Input =
            serde_json::from_str(&args[1]).expect("forwarded input payload should deserialize");
        assert_eq!(payload.name.as_deref(), Some("menu_image"));
        assert_eq!(payload.kind, crate::InputKind::Image);
        assert_eq!(payload.value.as_deref(), Some("./artifacts/menu.png"));
        assert!(notes.is_empty());
    }

    #[test]
    fn child_input_args_reject_unresolved_named_inputs() {
        let error = child_input_args(
            None,
            None,
            Some(&[crate::ActionInput::Named {
                input: "menu_image".to_string(),
            }]),
            &json!({}),
            "demo",
            &std::collections::BTreeMap::from([(
                "menu_image".to_string(),
                named_input("menu_image", crate::InputKind::Image, None),
            )]),
        )
        .unwrap_err();

        assert!(error.contains("required but unresolved"));
    }

    #[test]
    fn child_input_args_emit_named_override_flags_before_runtime_inputs() {
        let (args, notes) = child_input_args(
            Some(&[
                crate::ActionInputOverride {
                    name: "menu_note".to_string(),
                    value: crate::ActionInput::Text {
                        text: vec![crate::RunArg::Literal("spring menu".to_string())],
                    },
                },
                crate::ActionInputOverride {
                    name: "source_url".to_string(),
                    value: crate::ActionInput::Url {
                        url: vec![crate::RunArg::Literal(
                            "https://example.com/menu".to_string(),
                        )],
                    },
                },
            ]),
            Some(crate::ActionInputMode::Append),
            Some(&[crate::ActionInput::Text {
                text: vec![crate::RunArg::Literal("extra context".to_string())],
            }]),
            &json!({}),
            "demo",
            &no_named_inputs(),
        )
        .expect("child input args should resolve");

        assert_eq!(
            args,
            vec![
                "--input-override",
                "menu_note=spring menu",
                "--input-override",
                "source_url=https://example.com/menu",
                "--input-mode",
                "append",
                "--input-text",
                "extra context",
            ]
        );
        assert!(notes.is_empty());
    }

    #[test]
    fn child_input_args_support_parent_named_inputs_inside_input_overrides() {
        let (args, notes) = child_input_args(
            Some(&[crate::ActionInputOverride {
                name: "menu_image".to_string(),
                value: crate::ActionInput::Named {
                    input: "menu_image".to_string(),
                },
            }]),
            None,
            None,
            &json!({}),
            "demo",
            &std::collections::BTreeMap::from([(
                "menu_image".to_string(),
                named_input(
                    "menu_image",
                    crate::InputKind::Image,
                    Some("./artifacts/menu.png"),
                ),
            )]),
        )
        .expect("parent named input should resolve inside child override");

        assert_eq!(
            args,
            vec!["--input-override", "menu_image=./artifacts/menu.png"]
        );
        assert!(notes.is_empty());
    }

    #[test]
    fn child_input_args_resolve_dynamic_named_override_values() {
        let (args, notes) = child_input_args(
            Some(&[
                crate::ActionInputOverride {
                    name: "menu_note".to_string(),
                    value: crate::ActionInput::Text {
                        text: vec![
                            crate::RunArg::Literal("hello ".to_string()),
                            crate::RunArg::Variable("customer".to_string()),
                        ],
                    },
                },
                crate::ActionInputOverride {
                    name: "source_doc".to_string(),
                    value: crate::ActionInput::File {
                        path: vec![
                            crate::RunArg::Literal("./reports/".to_string()),
                            crate::RunArg::Variable("report_filename".to_string()),
                        ],
                    },
                },
            ]),
            None,
            None,
            &json!({
                "customer": "Acme",
                "report_filename": "q1.pdf"
            }),
            "demo",
            &no_named_inputs(),
        )
        .expect("dynamic named overrides should resolve");

        assert_eq!(
            args,
            vec![
                "--input-override",
                "menu_note=hello Acme",
                "--input-override",
                "source_doc=./reports/q1.pdf",
            ]
        );
        assert_eq!(notes.len(), 2);
        assert!(notes[0].contains("named text override 'menu_note'"));
        assert!(notes[1].contains("./reports/q1.pdf"));
    }

    #[test]
    fn child_input_args_reject_invalid_dynamic_named_override_url() {
        let error = child_input_args(
            Some(&[crate::ActionInputOverride {
                name: "source_url".to_string(),
                value: crate::ActionInput::Url {
                    url: vec![crate::RunArg::Variable("source_url".to_string())],
                },
            }]),
            None,
            None,
            &json!({
                "source_url": "ftp://example.com/report"
            }),
            "demo",
            &no_named_inputs(),
        )
        .unwrap_err();

        assert!(error.contains("named input override 'source_url'"));
        assert!(error.contains("absolute http(s) URL"));
    }

    #[test]
    fn child_input_args_reject_invalid_dynamic_named_override_file_extension() {
        let error = child_input_args(
            Some(&[crate::ActionInputOverride {
                name: "source_doc".to_string(),
                value: crate::ActionInput::File {
                    path: vec![
                        crate::RunArg::Literal("./reports/".to_string()),
                        crate::RunArg::Variable("report_filename".to_string()),
                    ],
                },
            }]),
            None,
            None,
            &json!({
                "report_filename": "q1.exe"
            }),
            "demo",
            &no_named_inputs(),
        )
        .unwrap_err();

        assert!(error.contains("named input override 'source_doc'"));
        assert!(error.contains("supported file extension"));
    }

    #[test]
    fn action_completion_summary_uses_completed_for_clean_runs() {
        let summary = action_completion_summary(&[StepExecutionOutcome::Completed]);
        assert_eq!(summary, Some("completed"));
    }

    #[test]
    fn action_completion_summary_suppresses_duplicate_single_step_email_success() {
        let summary = action_completion_summary(&[StepExecutionOutcome::SuccessAlreadyPrinted]);
        assert_eq!(summary, None);
    }

    #[test]
    fn action_completion_summary_suppresses_final_success_after_soft_failure() {
        let summary = action_completion_summary(&[
            StepExecutionOutcome::Completed,
            StepExecutionOutcome::SoftFailureLogged,
        ]);
        assert_eq!(summary, None);
    }

    #[test]
    fn run_completion_message_for_depth_prints_for_root_runs_only() {
        assert_eq!(
            run_completion_message_for_depth(0, std::time::Duration::from_secs(32)),
            Some("✅ Run complete in 32s.".to_string())
        );
        assert_eq!(
            run_completion_message_for_depth(1, std::time::Duration::from_secs(32)),
            None
        );
    }

    #[test]
    fn action_execution_header_uses_effective_mode() {
        assert_eq!(
            action_execution_header(crate::ActionExecutionMode::Sequential),
            "Action execution: sequential"
        );
        assert_eq!(
            action_execution_header(crate::ActionExecutionMode::Parallel),
            "Action execution: parallel"
        );
    }

    #[test]
    fn action_lane_prefix_uses_json_order_and_name() {
        assert_eq!(
            action_lane_prefix(0, "generate_images"),
            "[A1 generate_images]"
        );
        assert_eq!(action_lane_prefix(2, "child_summary"), "[A3 child_summary]");
    }

    #[test]
    fn live_dashboard_snapshot_shows_lane_state() {
        let output = ActionOutput::new_for_mode(
            crate::ActionExecutionMode::Parallel,
            ActionOutputMode::Live,
        );

        output.action_started(0, "generate_images");
        output.action_step_started(0, "generate_images", "generate_image", 1, 2);
        output.action_line(
            0,
            "generate_images",
            "wrote generated image to './artifacts/hero.png'.",
        );

        let snapshot = output.snapshot_lines_for_test();
        assert_eq!(snapshot[0], "Action execution: parallel");
        assert!(snapshot
            .iter()
            .any(|line| line.starts_with("[A1 generate_images] running · ")));
        assert!(snapshot
            .iter()
            .any(|line| line == "  step: 1/2 generate_image"));
        assert!(snapshot
            .iter()
            .any(|line| line == "  last: wrote generated image to './artifacts/hero.png'."));
        assert!(snapshot.iter().any(|line| line == "  output:"));
        assert!(snapshot
            .iter()
            .any(|line| line == "    wrote generated image to './artifacts/hero.png'."));
    }

    #[test]
    fn live_dashboard_snapshot_shows_waiting_message_for_long_running_step() {
        let output = ActionOutput::new_for_mode(
            crate::ActionExecutionMode::Parallel,
            ActionOutputMode::Live,
        );

        output.action_started(0, "generate_images");
        output.action_step_started(0, "generate_images", "generate_image", 2, 2);

        let snapshot = output.snapshot_lines_for_test();
        assert!(snapshot
            .iter()
            .any(|line| line.starts_with("[A1 generate_images] running · ")));
        assert!(snapshot
            .iter()
            .any(|line| line == "  step: 2/2 generate_image"));
        assert!(snapshot
            .iter()
            .any(|line| line == "  last: waiting for provider response..."));
    }

    #[test]
    fn live_dashboard_snapshot_marks_lane_completion_with_elapsed_time() {
        let output = ActionOutput::new_for_mode(
            crate::ActionExecutionMode::Sequential,
            ActionOutputMode::Live,
        );

        output.action_started(0, "generate_images");
        output.action_success(0, "generate_images", "completed");

        let snapshot = output.snapshot_lines_for_test();
        assert!(snapshot
            .iter()
            .any(|line| line.starts_with("[A1 generate_images] completed · ")));
        assert!(snapshot.iter().any(|line| line == "  step: ✓ done"));
        assert!(snapshot.iter().any(|line| line == "  last: completed."));
    }

    #[test]
    fn live_dashboard_snapshot_marks_lane_failures() {
        let output = ActionOutput::new_for_mode(
            crate::ActionExecutionMode::Sequential,
            ActionOutputMode::Live,
        );

        output.action_started(1, "child_summary");
        output.action_failed(1, "child_summary", "child exited with status 1");

        let snapshot = output.snapshot_lines_for_test();
        assert!(snapshot
            .iter()
            .any(|line| line.starts_with("[A2 child_summary] failed · ")));
        assert!(snapshot.iter().any(|line| line == "  step: ✗ failed"));
        assert!(snapshot
            .iter()
            .any(|line| line == "  last: failed: child exited with status 1"));
        assert!(snapshot.iter().any(|line| line == "  output:"));
        assert!(snapshot
            .iter()
            .any(|line| line == "    child exited with status 1"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_step_captures_output_variable_on_success() {
        let step = crate::RunStep {
            kind: "exec".to_string(),
            program: Some("/bin/sh".to_string()),
            model: None,
            profile: None,
            output_variable: Some("report_listing".to_string()),
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: vec![
                crate::RunArg::Literal("-lc".to_string()),
                crate::RunArg::Literal("printf 'alpha\\nbeta\\n'".to_string()),
            ],
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: None,
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let captured_output = run_exec_step(&step, &json!({}), 0, "capture_exec", runtime_budget)
            .await
            .expect("exec capture should succeed");

        assert_eq!(
            captured_output,
            Some(("report_listing".to_string(), "alpha\nbeta".to_string()))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_step_buckets_raw_output_into_live_lane() {
        let step = crate::RunStep {
            kind: "exec".to_string(),
            program: Some("/bin/sh".to_string()),
            model: None,
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: vec![
                crate::RunArg::Literal("-lc".to_string()),
                crate::RunArg::Literal("printf 'alpha\\n'; printf 'beta\\n' >&2".to_string()),
            ],
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: None,
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };
        let output = ActionOutput::new_for_mode(
            crate::ActionExecutionMode::Parallel,
            ActionOutputMode::Live,
        );
        output.action_started(0, "raw_exec");
        output.action_step_started(0, "raw_exec", "exec", 1, 1);

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = ACTION_OUTPUT
            .scope(output.clone(), async {
                run_exec_step(&step, &json!({}), 0, "raw_exec", runtime_budget).await
            })
            .await;

        assert!(result.is_ok(), "raw exec step should succeed: {result:?}");
        output.action_success(0, "raw_exec", "completed");

        let snapshot = output.snapshot_lines_for_test();
        assert!(snapshot
            .iter()
            .any(|line| line.starts_with("[A1 raw_exec] completed · ")));
        assert!(snapshot.iter().any(|line| line == "  step: ✓ done"));
        assert!(snapshot.iter().any(|line| line == "    alpha"));
        assert!(snapshot.iter().any(|line| line == "    beta"));
    }

    #[tokio::test]
    async fn generate_image_step_writes_single_output_file() {
        use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};

        let mut server = mockito::Server::new_async().await;
        let expected_bytes = b"fake-png";
        let encoded_image = BASE64_STANDARD.encode(expected_bytes);
        let _mock = server
            .mock("POST", "/v1/images/generations")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"data":[{{"b64_json":"{}"}}]}}"#,
                encoded_image
            ))
            .create_async()
            .await;

        let output_name = format!(".tmp-cai2054-generated-image-{}.png", std::process::id());
        let step = crate::RunStep {
            kind: "generate_image".to_string(),
            program: None,
            model: Some(crate::RunArg::Literal("gpt-image-1".to_string())),
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: Some(vec![
                crate::RunArg::Literal("Create an image for ".to_string()),
                crate::RunArg::Variable("customer".to_string()),
            ]),
            path: Some(vec![crate::RunArg::Literal(output_name.clone())]),
            subject: None,
            text: None,
            agent: None,
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };
        let provider_context = ActionProviderContext {
            provider: ProviderKind::OpenAi,
            model: "gpt-5.2".to_string(),
            url: format!("{}/v1/chat/completions", server.url()),
            token: "test-token".to_string(),
            inference_timeout_in_sec: 60,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = run_generate_image_step(
            &step,
            &json!({ "customer": "Acme" }),
            0,
            "generate_art",
            &provider_context,
            runtime_budget,
        )
        .await;

        assert!(
            result.is_ok(),
            "image generation should succeed: {result:?}"
        );

        let written_bytes =
            std::fs::read(&output_name).expect("generated image file should be written");
        let _ = std::fs::remove_file(&output_name);
        assert_eq!(written_bytes, expected_bytes);
    }

    #[tokio::test]
    async fn generate_image_step_resolves_model_from_runtime_variable() {
        use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};

        let mut server = mockito::Server::new_async().await;
        let expected_bytes = b"fake-png-runtime";
        let encoded_image = BASE64_STANDARD.encode(expected_bytes);
        let _mock = server
            .mock("POST", "/v1/images/generations")
            .match_body(mockito::Matcher::PartialJson(
                json!({ "model": "gpt-image-1" }),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"data":[{{"b64_json":"{}"}}]}}"#,
                encoded_image
            ))
            .create_async()
            .await;

        let output_name = format!(
            ".tmp-cai2055-generated-image-runtime-{}.png",
            std::process::id()
        );
        let step = crate::RunStep {
            kind: "generate_image".to_string(),
            program: None,
            model: Some(crate::RunArg::Variable("runtime.image_model".to_string())),
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: Some(vec![crate::RunArg::Literal(
                "Create an image for Acme".to_string(),
            )]),
            path: Some(vec![crate::RunArg::Literal(output_name.clone())]),
            subject: None,
            text: None,
            agent: None,
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };
        let provider_context = ActionProviderContext {
            provider: ProviderKind::OpenAi,
            model: "gpt-5.2".to_string(),
            url: format!("{}/v1/chat/completions", server.url()),
            token: "test-token".to_string(),
            inference_timeout_in_sec: 60,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = run_generate_image_step(
            &step,
            &json!({
                "runtime": {
                    "image_model": "gpt-image-1"
                }
            }),
            0,
            "generate_art",
            &provider_context,
            runtime_budget,
        )
        .await;

        assert!(
            result.is_ok(),
            "runtime-image generation should succeed: {result:?}"
        );

        let written_bytes =
            std::fs::read(&output_name).expect("generated image file should be written");
        let _ = std::fs::remove_file(&output_name);
        assert_eq!(written_bytes, expected_bytes);
    }

    #[tokio::test]
    async fn generate_image_step_falls_back_to_effective_invocation_model() {
        use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};

        let mut server = mockito::Server::new_async().await;
        let expected_bytes = b"fake-png-fallback";
        let encoded_image = BASE64_STANDARD.encode(expected_bytes);
        let _mock = server
            .mock("POST", "/v1/images/generations")
            .match_body(mockito::Matcher::PartialJson(
                json!({ "model": "gpt-image-1.5" }),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"data":[{{"b64_json":"{}"}}]}}"#,
                encoded_image
            ))
            .create_async()
            .await;

        let output_name = format!(
            ".tmp-cai2067-generated-image-fallback-{}.png",
            std::process::id()
        );
        let step = crate::RunStep {
            kind: "generate_image".to_string(),
            program: None,
            model: None,
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: Some(vec![crate::RunArg::Literal(
                "Create an image for Acme".to_string(),
            )]),
            path: Some(vec![crate::RunArg::Literal(output_name.clone())]),
            subject: None,
            text: None,
            agent: None,
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };
        let provider_context = ActionProviderContext {
            provider: ProviderKind::OpenAi,
            model: "gpt-image-1.5".to_string(),
            url: format!("{}/v1/chat/completions", server.url()),
            token: "test-token".to_string(),
            inference_timeout_in_sec: 60,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = run_generate_image_step(
            &step,
            &json!({}),
            0,
            "generate_art",
            &provider_context,
            runtime_budget,
        )
        .await;

        assert!(
            result.is_ok(),
            "fallback-image generation should succeed: {result:?}"
        );

        let written_bytes =
            std::fs::read(&output_name).expect("generated image file should be written");
        let _ = std::fs::remove_file(&output_name);
        assert_eq!(written_bytes, expected_bytes);
    }

    #[tokio::test]
    async fn generate_image_step_requires_model_when_step_and_invocation_omit_it() {
        let step = crate::RunStep {
            kind: "generate_image".to_string(),
            program: None,
            model: None,
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: Some(vec![crate::RunArg::Literal(
                "Create an image for Acme".to_string(),
            )]),
            path: Some(vec![crate::RunArg::Literal(
                "./artifacts/missing-model.png".to_string(),
            )]),
            subject: None,
            text: None,
            agent: None,
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };
        let provider_context = ActionProviderContext {
            provider: ProviderKind::OpenAi,
            model: String::new(),
            url: "https://api.openai.com/v1/chat/completions".to_string(),
            token: "test-token".to_string(),
            inference_timeout_in_sec: 60,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let error = run_generate_image_step(
            &step,
            &json!({}),
            0,
            "generate_art",
            &provider_context,
            runtime_budget,
        )
        .await
        .expect_err("missing step and invocation model should fail");

        assert!(error.contains("omitted `model`"));
        assert!(error.contains("pass `--model`"));
    }

    #[tokio::test]
    async fn generate_image_step_uses_step_profile_model_when_explicit_model_omitted() {
        use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};

        let mut server = mockito::Server::new_async().await;
        let expected_bytes = b"fake-png-step-profile";
        let encoded_image = BASE64_STANDARD.encode(expected_bytes);
        let _mock = server
            .mock("POST", "/v1/images/generations")
            .match_body(mockito::Matcher::PartialJson(
                json!({ "model": "gpt-image-step-profile" }),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"data":[{{"b64_json":"{}"}}]}}"#,
                encoded_image
            ))
            .create_async()
            .await;

        let config = profile_config(
            "image_profile",
            format!("{}/v1/chat/completions", server.url()).as_str(),
            "gpt-image-step-profile",
        );
        let _test_env = TestCargoHome::new(&config);

        let output_name = format!(
            ".tmp-cai2067-generated-image-step-profile-{}.png",
            std::process::id()
        );
        let step = crate::RunStep {
            kind: "generate_image".to_string(),
            program: None,
            model: None,
            profile: Some(crate::RunArg::Literal("image_profile".to_string())),
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: Some(vec![crate::RunArg::Literal(
                "Create an image for Acme".to_string(),
            )]),
            path: Some(vec![crate::RunArg::Literal(output_name.clone())]),
            subject: None,
            text: None,
            agent: None,
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };
        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = run_generate_image_step(
            &step,
            &json!({}),
            0,
            "generate_art",
            &provider_context(),
            runtime_budget,
        )
        .await;

        assert!(
            result.is_ok(),
            "profile-backed image generation should succeed: {result:?}"
        );

        let written_bytes =
            std::fs::read(&output_name).expect("generated image file should be written");
        let _ = std::fs::remove_file(&output_name);
        assert_eq!(written_bytes, expected_bytes);
    }

    #[tokio::test]
    async fn generate_image_step_explicit_model_overrides_step_profile_model() {
        use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};

        let mut server = mockito::Server::new_async().await;
        let expected_bytes = b"fake-png-explicit-step-model";
        let encoded_image = BASE64_STANDARD.encode(expected_bytes);
        let _mock = server
            .mock("POST", "/v1/images/generations")
            .match_body(mockito::Matcher::PartialJson(
                json!({ "model": "gpt-image-explicit" }),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"data":[{{"b64_json":"{}"}}]}}"#,
                encoded_image
            ))
            .create_async()
            .await;

        let config = profile_config(
            "image_profile",
            format!("{}/v1/chat/completions", server.url()).as_str(),
            "gpt-image-step-profile",
        );
        let _test_env = TestCargoHome::new(&config);

        let output_name = format!(
            ".tmp-cai2067-generated-image-step-explicit-{}.png",
            std::process::id()
        );
        let step = crate::RunStep {
            kind: "generate_image".to_string(),
            program: None,
            model: Some(crate::RunArg::Literal("gpt-image-explicit".to_string())),
            profile: Some(crate::RunArg::Literal("image_profile".to_string())),
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: Some(vec![crate::RunArg::Literal(
                "Create an image for Acme".to_string(),
            )]),
            path: Some(vec![crate::RunArg::Literal(output_name.clone())]),
            subject: None,
            text: None,
            agent: None,
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };
        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = run_generate_image_step(
            &step,
            &json!({}),
            0,
            "generate_art",
            &provider_context(),
            runtime_budget,
        )
        .await;

        assert!(
            result.is_ok(),
            "explicit-model image generation should succeed: {result:?}"
        );

        let written_bytes =
            std::fs::read(&output_name).expect("generated image file should be written");
        let _ = std::fs::remove_file(&output_name);
        assert_eq!(written_bytes, expected_bytes);
    }

    #[tokio::test]
    async fn generate_image_step_rejects_unknown_step_profile() {
        let config = profile_config(
            "other_profile",
            "https://api.openai.com/v1/chat/completions",
            "gpt-image-step-profile",
        );
        let _test_env = TestCargoHome::new(&config);

        let step = crate::RunStep {
            kind: "generate_image".to_string(),
            program: None,
            model: Some(crate::RunArg::Literal("gpt-image-explicit".to_string())),
            profile: Some(crate::RunArg::Literal("missing_profile".to_string())),
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: Some(vec![crate::RunArg::Literal(
                "Create an image for Acme".to_string(),
            )]),
            path: Some(vec![crate::RunArg::Literal(
                "./artifacts/missing-profile.png".to_string(),
            )]),
            subject: None,
            text: None,
            agent: None,
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };
        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let error = run_generate_image_step(
            &step,
            &json!({}),
            0,
            "generate_art",
            &provider_context(),
            runtime_budget,
        )
        .await
        .expect_err("missing profile should fail");

        assert!(error.contains("unknown profile 'missing_profile'"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_step_invokes_child_with_forwarded_inputs() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2032-agent-child-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);
        let output_path = std::env::temp_dir().join(format!(
            "cai2032-agent-child-args-{}.txt",
            std::process::id()
        ));

        let script_body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n",
            output_path.display()
        );

        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            model: None,
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: Some(format!("./{}", script_name)),
            input_overrides: None,
            inputs: Some(vec![
                crate::ActionInput::Text {
                    text: vec![
                        crate::RunArg::Literal("hello ".to_string()),
                        crate::RunArg::Variable("customer".to_string()),
                    ],
                },
                crate::ActionInput::Url {
                    url: vec![crate::RunArg::Literal("https://example.com".to_string())],
                },
                crate::ActionInput::Image {
                    path: vec![crate::RunArg::Literal("./diagram.png".to_string())],
                },
                crate::ActionInput::File {
                    path: vec![
                        crate::RunArg::Literal("./reports/".to_string()),
                        crate::RunArg::Variable("report_filename".to_string()),
                    ],
                },
            ]),
            input_mode: Some(crate::ActionInputMode::Append),
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = run_agent_step(
            &step,
            &json!({
                "customer": "world",
                "report_filename": "report.pdf"
            }),
            &no_named_inputs(),
            0,
            "invoke_child",
            None,
            5,
            runtime_budget,
        )
        .await;

        let _ = fs::remove_file(&script_path);

        assert!(
            result.is_ok(),
            "child agent invocation should succeed: {result:?}"
        );

        let args = fs::read_to_string(&output_path).expect("child output should be captured");
        let _ = fs::remove_file(&output_path);

        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            vec![
                "--input-mode",
                "append",
                "--input-text",
                "hello world",
                "--input-url",
                "https://example.com",
                "--input-image",
                "./diagram.png",
                "--input-file",
                "./reports/report.pdf",
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_step_summarizes_child_output_without_inlining_child_transcript() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2067-child-summary-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);
        let marker_path = std::env::temp_dir().join(format!(
            "cai2067-child-summary-marker-{}.txt",
            std::process::id()
        ));

        let script_body = format!(
            "#!/bin/sh\nprintf 'child detail\\n'\nprintf 'ran' > \"{}\"\n",
            marker_path.display()
        );
        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            model: None,
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: Some(format!("./{}", script_name)),
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let output = ActionOutput::new_for_mode(
            crate::ActionExecutionMode::Parallel,
            ActionOutputMode::Live,
        );
        output.action_started(0, "child_summary");
        output.action_step_started(0, "child_summary", "agent", 1, 1);

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = ACTION_OUTPUT
            .scope(output.clone(), async {
                run_agent_step(
                    &step,
                    &json!({}),
                    &no_named_inputs(),
                    0,
                    "child_summary",
                    None,
                    5,
                    runtime_budget,
                )
                .await
            })
            .await;

        let _ = fs::remove_file(&script_path);
        let marker =
            fs::read_to_string(&marker_path).expect("child script should still execute normally");
        let _ = fs::remove_file(&marker_path);
        assert_eq!(marker, "ran");

        assert!(
            result.is_ok(),
            "child summary step should succeed without passthrough: {result:?}"
        );
        output.action_success(0, "child_summary", "completed");

        let snapshot = output.snapshot_lines_for_test();
        assert!(snapshot
            .iter()
            .any(|line| line.starts_with("[A1 child_summary] completed · ")));
        assert!(snapshot.iter().any(|line| line == "  step: ✓ done"));
        assert!(snapshot
            .iter()
            .any(|line| line == &format!("    child: started ./{}", script_name)));
        assert!(snapshot
            .iter()
            .any(|line| line == "    child: completed successfully"));
        assert!(!snapshot.iter().any(|line| line.contains("child detail")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_step_forwards_action_execution_override_to_child() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(
            ".tmp-cai2067-action-execution-child-{}.sh",
            std::process::id()
        );
        let script_path = current_dir.join(&script_name);
        let output_path = std::env::temp_dir().join(format!(
            "cai2067-action-execution-child-args-{}.txt",
            std::process::id()
        ));

        let script_body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n",
            output_path.display()
        );

        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            model: None,
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: Some(format!("./{}", script_name)),
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = run_agent_step(
            &step,
            &json!({}),
            &no_named_inputs(),
            0,
            "invoke_child",
            Some(crate::ActionExecutionMode::Sequential),
            5,
            runtime_budget,
        )
        .await;

        let _ = fs::remove_file(&script_path);

        assert!(
            result.is_ok(),
            "child agent invocation should succeed: {result:?}"
        );

        let args = fs::read_to_string(&output_path).expect("child output should be captured");
        let _ = fs::remove_file(&output_path);

        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            vec!["--action-execution", "sequential"]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_step_forwards_step_profile_to_child() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let config = profile_config(
            "child_profile",
            "https://api.openai.com/v1/chat/completions",
            "gpt-5.2",
        );
        let _test_env = TestCargoHome::new(&config);

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2067-child-profile-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);
        let output_path = std::env::temp_dir().join(format!(
            "cai2067-child-profile-args-{}.txt",
            std::process::id()
        ));

        let script_body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n",
            output_path.display()
        );

        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            model: None,
            profile: Some(crate::RunArg::Variable("runtime.child_profile".to_string())),
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: Some(format!("./{}", script_name)),
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = run_agent_step(
            &step,
            &json!({
                "runtime": {
                    "child_profile": "child_profile"
                }
            }),
            &no_named_inputs(),
            0,
            "invoke_child",
            Some(crate::ActionExecutionMode::Sequential),
            5,
            runtime_budget,
        )
        .await;

        let _ = fs::remove_file(&script_path);

        assert!(
            result.is_ok(),
            "child agent invocation should succeed: {result:?}"
        );

        let args = fs::read_to_string(&output_path).expect("child output should be captured");
        let _ = fs::remove_file(&output_path);

        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            vec![
                "--action-execution",
                "sequential",
                "--profile",
                "child_profile",
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn captured_exec_output_can_flow_into_later_agent_step() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2036-phase5-child-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);
        let output_path = std::env::temp_dir().join(format!(
            "cai2036-phase5-child-args-{}.txt",
            std::process::id()
        ));

        let script_body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n",
            output_path.display()
        );

        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let exec_step = crate::RunStep {
            kind: "exec".to_string(),
            program: Some("/bin/sh".to_string()),
            model: None,
            profile: None,
            output_variable: Some("report_listing".to_string()),
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: vec![
                crate::RunArg::Literal("-lc".to_string()),
                crate::RunArg::Literal("printf 'q1.pdf | q2.pdf\\n'".to_string()),
            ],
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: None,
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };
        let agent_step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            model: None,
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: Some(format!("./{}", script_name)),
            input_overrides: None,
            inputs: Some(vec![crate::ActionInput::Text {
                text: vec![
                    crate::RunArg::Literal("Files:\n".to_string()),
                    crate::RunArg::Variable("report_listing".to_string()),
                ],
            }]),
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let mut action_data = json!({});
        let captured_output = run_exec_step(
            &exec_step,
            &action_data,
            0,
            "capture_then_agent",
            runtime_budget,
        )
        .await
        .expect("exec capture should succeed");
        let (name, value) = captured_output.expect("captured output should be present");
        insert_action_output_variable(&mut action_data, name.as_str(), value, "capture_then_agent")
            .expect("captured output should be inserted");

        let result = run_agent_step(
            &agent_step,
            &action_data,
            &no_named_inputs(),
            0,
            "capture_then_agent",
            None,
            5,
            runtime_budget,
        )
        .await;

        let _ = fs::remove_file(&script_path);

        assert!(
            result.is_ok(),
            "child agent invocation should succeed: {result:?}"
        );

        let args = fs::read_to_string(&output_path).expect("child output should be captured");
        let _ = fs::remove_file(&output_path);

        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            vec!["--input-text", "Files:", "q1.pdf | q2.pdf"]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_step_inherits_max_depth_and_runtime_budget_for_child_processes() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2032-agent-depth-child-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);
        let output_path =
            std::env::temp_dir().join(format!("cai2032-agent-depth-{}.txt", std::process::id()));

        let script_body = format!(
            "#!/bin/sh\nprintf '%s\\n%s' \"$CARGO_AI_AGENT_ACTION_MAX_DEPTH\" \"$CARGO_AI_AGENT_MAX_RUNTIME_SECS\" > \"{}\"\n",
            output_path.display()
        );

        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            model: None,
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: Some(format!("./{}", script_name)),
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = run_agent_step(
            &step,
            &json!({}),
            &no_named_inputs(),
            0,
            "invoke_child",
            None,
            7,
            runtime_budget,
        )
        .await;

        let _ = fs::remove_file(&script_path);

        assert!(
            result.is_ok(),
            "child agent invocation should succeed: {result:?}"
        );

        let inherited_values =
            fs::read_to_string(&output_path).expect("child output should be captured");
        let _ = fs::remove_file(&output_path);

        assert_eq!(
            inherited_values.lines().collect::<Vec<_>>(),
            vec!["7", "600"]
        );
    }

    #[tokio::test]
    async fn agent_step_rejects_bare_child_name() {
        let step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            model: None,
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: Some("child_agent".to_string()),
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let error = run_agent_step(
            &step,
            &json!({}),
            &no_named_inputs(),
            0,
            "invoke_child",
            None,
            5,
            runtime_budget,
        )
        .await
        .expect_err("bare child agent names should be rejected");

        assert!(error.contains("bare child-agent names are not allowed"));
    }

    #[tokio::test]
    async fn agent_step_rejects_parent_traversal_path() {
        let step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            model: None,
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: Some("./../child_agent".to_string()),
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let error = run_agent_step(
            &step,
            &json!({}),
            &no_named_inputs(),
            0,
            "invoke_child",
            None,
            5,
            runtime_budget,
        )
        .await
        .expect_err("parent traversal should be rejected");

        assert!(error.contains("parent traversal"));
    }

    #[tokio::test]
    async fn agent_step_rejects_nested_child_path() {
        let step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            model: None,
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: Some("./agents/child_agent".to_string()),
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let error = run_agent_step(
            &step,
            &json!({}),
            &no_named_inputs(),
            0,
            "invoke_child",
            None,
            5,
            runtime_budget,
        )
        .await
        .expect_err("nested child agent paths should be rejected");

        assert!(error.contains("nested child-agent paths"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_step_times_out_against_invocation_budget() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2032-agent-timeout-child-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);

        let script_body = "#!/bin/sh\nsleep 2\n";

        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            model: None,
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: Some(format!("./{}", script_name)),
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(1));
        let error = run_agent_step(
            &step,
            &json!({}),
            &no_named_inputs(),
            0,
            "invoke_child",
            None,
            5,
            runtime_budget,
        )
        .await
        .expect_err("runtime budget should time out the child");

        let _ = fs::remove_file(&script_path);

        assert!(error.contains("max-runtime-in-sec 1"));
        assert!(error.contains("while waiting for child agent"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn apply_actions_stops_on_failed_exec_by_default() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2040-stop-child-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);
        let output_path =
            std::env::temp_dir().join(format!("cai2040-stop-output-{}.txt", std::process::id()));

        let script_body = format!("#!/bin/sh\nprintf 'ran' > \"{}\"\n", output_path.display());
        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let failing_step = crate::RunStep {
            kind: "exec".to_string(),
            program: Some("/bin/sh".to_string()),
            model: None,
            profile: None,
            output_variable: None,
            status_variable: Some("step_status".to_string()),
            error_variable: Some("step_error".to_string()),
            failure_mode: None,
            when: None,
            args: vec![
                crate::RunArg::Literal("-lc".to_string()),
                crate::RunArg::Literal("exit 7".to_string()),
            ],
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: None,
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };
        let second_step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            model: None,
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: Some(format!("./{}", script_name)),
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = apply_actions(
            &crate::Output { answer: 4 },
            &[action(vec![failing_step, second_step])],
            &serde_json::Map::new(),
            &[],
            crate::ActionExecutionMode::Sequential,
            None,
            &provider_context(),
            5,
            runtime_budget,
        )
        .await;

        let _ = fs::remove_file(&script_path);
        let _ = fs::remove_file(&output_path);

        let error = result.expect_err("failed exec should stop by default");
        assert!(error.contains("exited with status"));
        assert!(
            !output_path.exists(),
            "later step should not run after default stop"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn apply_actions_continues_to_later_top_level_actions_after_hard_failure() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2067-later-action-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);
        let output_path =
            std::env::temp_dir().join(format!("cai2067-later-action-{}.txt", std::process::id()));

        let script_body = format!("#!/bin/sh\nprintf 'ran' > \"{}\"\n", output_path.display());
        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let first_action = crate::Action {
            name: "first_action".to_string(),
            logic: json!({ "==": [{ "var": "answer" }, 4] }),
            run: vec![crate::RunStep {
                kind: "exec".to_string(),
                program: Some("/bin/sh".to_string()),
                model: None,
                profile: None,
                output_variable: None,
                status_variable: None,
                error_variable: None,
                failure_mode: None,
                when: None,
                args: vec![
                    crate::RunArg::Literal("-lc".to_string()),
                    crate::RunArg::Literal("exit 11".to_string()),
                ],
                prompt: None,
                path: None,
                subject: None,
                text: None,
                agent: None,
                input_overrides: None,
                inputs: None,
                input_mode: None,
                platforms: None,
            }],
        };
        let second_action = crate::Action {
            name: "second_action".to_string(),
            logic: json!({ "==": [{ "var": "answer" }, 4] }),
            run: vec![crate::RunStep {
                kind: "agent".to_string(),
                program: None,
                model: None,
                profile: None,
                output_variable: None,
                status_variable: None,
                error_variable: None,
                failure_mode: None,
                when: None,
                args: Vec::new(),
                prompt: None,
                path: None,
                subject: None,
                text: None,
                agent: Some(format!("./{}", script_name)),
                input_overrides: None,
                inputs: None,
                input_mode: None,
                platforms: None,
            }],
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = apply_actions(
            &crate::Output { answer: 4 },
            &[first_action, second_action],
            &serde_json::Map::new(),
            &[],
            crate::ActionExecutionMode::Sequential,
            None,
            &provider_context(),
            5,
            runtime_budget,
        )
        .await;

        let _ = fs::remove_file(&script_path);

        let error = result.expect_err("one failed action should still fail overall");
        assert!(error.contains("first_action"));
        assert!(error.contains("exited with status"));

        let file_contents =
            fs::read_to_string(&output_path).expect("later top-level action should have executed");
        let _ = fs::remove_file(&output_path);
        assert_eq!(file_contents, "ran");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn apply_actions_parallel_starts_matching_actions_without_waiting() {
        use std::fs;

        let first_output_path =
            std::env::temp_dir().join(format!("cai2067-parallel-first-{}.txt", std::process::id()));
        let second_output_path = std::env::temp_dir().join(format!(
            "cai2067-parallel-second-{}.txt",
            std::process::id()
        ));

        let first_action = crate::Action {
            name: "first_action".to_string(),
            logic: json!({ "==": [{ "var": "answer" }, 4] }),
            run: vec![crate::RunStep {
                kind: "exec".to_string(),
                program: Some("/bin/sh".to_string()),
                model: None,
                profile: None,
                output_variable: None,
                status_variable: None,
                error_variable: None,
                failure_mode: None,
                when: None,
                args: vec![
                    crate::RunArg::Literal("-lc".to_string()),
                    crate::RunArg::Literal(format!(
                        "sleep 1; printf 'first' > \"{}\"",
                        first_output_path.display()
                    )),
                ],
                prompt: None,
                path: None,
                subject: None,
                text: None,
                agent: None,
                input_overrides: None,
                inputs: None,
                input_mode: None,
                platforms: None,
            }],
        };
        let second_action = crate::Action {
            name: "second_action".to_string(),
            logic: json!({ "==": [{ "var": "answer" }, 4] }),
            run: vec![crate::RunStep {
                kind: "exec".to_string(),
                program: Some("/bin/sh".to_string()),
                model: None,
                profile: None,
                output_variable: None,
                status_variable: None,
                error_variable: None,
                failure_mode: None,
                when: None,
                args: vec![
                    crate::RunArg::Literal("-lc".to_string()),
                    crate::RunArg::Literal(format!(
                        "printf 'second' > \"{}\"",
                        second_output_path.display()
                    )),
                ],
                prompt: None,
                path: None,
                subject: None,
                text: None,
                agent: None,
                input_overrides: None,
                inputs: None,
                input_mode: None,
                platforms: None,
            }],
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let output = crate::Output { answer: 4 };
        let actions = vec![first_action, second_action];
        let runtime_vars = serde_json::Map::new();
        let provider_context = provider_context();
        let mut future = std::pin::pin!(apply_actions(
            &output,
            &actions,
            &runtime_vars,
            &[],
            crate::ActionExecutionMode::Parallel,
            None,
            &provider_context,
            5,
            runtime_budget,
        ));

        tokio::select! {
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(300)) => {}
            result = &mut future => panic!("parallel actions finished too early: {result:?}"),
        }

        assert!(
            second_output_path.exists(),
            "second action should run before the delayed first action completes"
        );
        assert!(
            !first_output_path.exists(),
            "first action should still be sleeping when the second action has already completed"
        );

        let result = future.await;

        let first_contents =
            fs::read_to_string(&first_output_path).expect("first action should eventually finish");
        let second_contents = fs::read_to_string(&second_output_path)
            .expect("second action should have already completed");
        let _ = fs::remove_file(&first_output_path);
        let _ = fs::remove_file(&second_output_path);

        assert!(
            result.is_ok(),
            "parallel actions should succeed: {result:?}"
        );
        assert_eq!(first_contents, "first");
        assert_eq!(second_contents, "second");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn apply_actions_parallel_reports_failures_in_lane_order() {
        let first_action = crate::Action {
            name: "first_action".to_string(),
            logic: json!({ "==": [{ "var": "answer" }, 4] }),
            run: vec![crate::RunStep {
                kind: "exec".to_string(),
                program: Some("/bin/sh".to_string()),
                model: None,
                profile: None,
                output_variable: None,
                status_variable: None,
                error_variable: None,
                failure_mode: None,
                when: None,
                args: vec![
                    crate::RunArg::Literal("-lc".to_string()),
                    crate::RunArg::Literal("sleep 1; exit 11".to_string()),
                ],
                prompt: None,
                path: None,
                subject: None,
                text: None,
                agent: None,
                input_overrides: None,
                inputs: None,
                input_mode: None,
                platforms: None,
            }],
        };
        let second_action = crate::Action {
            name: "second_action".to_string(),
            logic: json!({ "==": [{ "var": "answer" }, 4] }),
            run: vec![crate::RunStep {
                kind: "exec".to_string(),
                program: Some("/bin/sh".to_string()),
                model: None,
                profile: None,
                output_variable: None,
                status_variable: None,
                error_variable: None,
                failure_mode: None,
                when: None,
                args: vec![
                    crate::RunArg::Literal("-lc".to_string()),
                    crate::RunArg::Literal("exit 12".to_string()),
                ],
                prompt: None,
                path: None,
                subject: None,
                text: None,
                agent: None,
                input_overrides: None,
                inputs: None,
                input_mode: None,
                platforms: None,
            }],
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let actions = vec![first_action, second_action];
        let error = apply_actions(
            &crate::Output { answer: 4 },
            &actions,
            &serde_json::Map::new(),
            &[],
            crate::ActionExecutionMode::Parallel,
            None,
            &provider_context(),
            5,
            runtime_budget,
        )
        .await
        .expect_err("parallel hard failures should fail overall");

        let first_idx = error
            .find("first_action")
            .expect("aggregated error should mention first lane");
        let second_idx = error
            .find("second_action")
            .expect("aggregated error should mention second lane");

        assert!(
            first_idx < second_idx,
            "parallel failure reporting should remain in JSON/lane order: {error}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn apply_actions_abort_stops_current_and_later_top_level_work() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2067-abort-child-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);
        let output_path =
            std::env::temp_dir().join(format!("cai2067-abort-output-{}.txt", std::process::id()));

        let script_body = format!("#!/bin/sh\nprintf 'ran' > \"{}\"\n", output_path.display());
        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let first_action = crate::Action {
            name: "first_action".to_string(),
            logic: json!({ "==": [{ "var": "answer" }, 4] }),
            run: vec![
                crate::RunStep {
                    kind: "exec".to_string(),
                    program: Some("/bin/sh".to_string()),
                    model: None,
                    profile: None,
                    output_variable: None,
                    status_variable: Some("abort_status".to_string()),
                    error_variable: Some("abort_error".to_string()),
                    failure_mode: Some(crate::FailureMode::Abort),
                    when: None,
                    args: vec![
                        crate::RunArg::Literal("-lc".to_string()),
                        crate::RunArg::Literal("exit 17".to_string()),
                    ],
                    prompt: None,
                    path: None,
                    subject: None,
                    text: None,
                    agent: None,
                    input_overrides: None,
                    inputs: None,
                    input_mode: None,
                    platforms: None,
                },
                crate::RunStep {
                    kind: "agent".to_string(),
                    program: None,
                    model: None,
                    profile: None,
                    output_variable: None,
                    status_variable: None,
                    error_variable: None,
                    failure_mode: None,
                    when: None,
                    args: Vec::new(),
                    prompt: None,
                    path: None,
                    subject: None,
                    text: None,
                    agent: Some(format!("./{}", script_name)),
                    input_overrides: None,
                    inputs: None,
                    input_mode: None,
                    platforms: None,
                },
            ],
        };
        let second_action = crate::Action {
            name: "second_action".to_string(),
            logic: json!({ "==": [{ "var": "answer" }, 4] }),
            run: vec![crate::RunStep {
                kind: "agent".to_string(),
                program: None,
                model: None,
                profile: None,
                output_variable: None,
                status_variable: None,
                error_variable: None,
                failure_mode: None,
                when: None,
                args: Vec::new(),
                prompt: None,
                path: None,
                subject: None,
                text: None,
                agent: Some(format!("./{}", script_name)),
                input_overrides: None,
                inputs: None,
                input_mode: None,
                platforms: None,
            }],
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let error = apply_actions(
            &crate::Output { answer: 4 },
            &[first_action, second_action],
            &serde_json::Map::new(),
            &[],
            crate::ActionExecutionMode::Sequential,
            None,
            &provider_context(),
            5,
            runtime_budget,
        )
        .await
        .expect_err("abort should fail the invocation");

        let output_exists = output_path.exists();
        let _ = fs::remove_file(&script_path);
        let _ = fs::remove_file(&output_path);

        assert!(error.contains("Run aborted by [A1 first_action]"));
        assert!(error.contains("exit status: 17"));
        assert!(
            !output_exists,
            "abort should stop later steps in the lane and later top-level actions"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn apply_actions_parallel_abort_stops_other_lanes_before_next_step() {
        use std::fs;

        let later_output_path = std::env::temp_dir().join(format!(
            "cai2067-parallel-abort-output-{}.txt",
            std::process::id()
        ));

        let first_action = crate::Action {
            name: "first_action".to_string(),
            logic: json!({ "==": [{ "var": "answer" }, 4] }),
            run: vec![
                crate::RunStep {
                    kind: "exec".to_string(),
                    program: Some("/bin/sh".to_string()),
                    model: None,
                    profile: None,
                    output_variable: None,
                    status_variable: None,
                    error_variable: None,
                    failure_mode: None,
                    when: None,
                    args: vec![
                        crate::RunArg::Literal("-lc".to_string()),
                        crate::RunArg::Literal("sleep 0.1".to_string()),
                    ],
                    prompt: None,
                    path: None,
                    subject: None,
                    text: None,
                    agent: None,
                    input_overrides: None,
                    inputs: None,
                    input_mode: None,
                    platforms: None,
                },
                crate::RunStep {
                    kind: "exec".to_string(),
                    program: Some("/bin/sh".to_string()),
                    model: None,
                    profile: None,
                    output_variable: None,
                    status_variable: None,
                    error_variable: None,
                    failure_mode: Some(crate::FailureMode::Abort),
                    when: None,
                    args: vec![
                        crate::RunArg::Literal("-lc".to_string()),
                        crate::RunArg::Literal("exit 19".to_string()),
                    ],
                    prompt: None,
                    path: None,
                    subject: None,
                    text: None,
                    agent: None,
                    input_overrides: None,
                    inputs: None,
                    input_mode: None,
                    platforms: None,
                },
            ],
        };
        let second_action = crate::Action {
            name: "second_action".to_string(),
            logic: json!({ "==": [{ "var": "answer" }, 4] }),
            run: vec![
                crate::RunStep {
                    kind: "exec".to_string(),
                    program: Some("/bin/sh".to_string()),
                    model: None,
                    profile: None,
                    output_variable: None,
                    status_variable: None,
                    error_variable: None,
                    failure_mode: None,
                    when: None,
                    args: vec![
                        crate::RunArg::Literal("-lc".to_string()),
                        crate::RunArg::Literal("sleep 0.2".to_string()),
                    ],
                    prompt: None,
                    path: None,
                    subject: None,
                    text: None,
                    agent: None,
                    input_overrides: None,
                    inputs: None,
                    input_mode: None,
                    platforms: None,
                },
                crate::RunStep {
                    kind: "exec".to_string(),
                    program: Some("/bin/sh".to_string()),
                    model: None,
                    profile: None,
                    output_variable: None,
                    status_variable: None,
                    error_variable: None,
                    failure_mode: None,
                    when: None,
                    args: vec![
                        crate::RunArg::Literal("-lc".to_string()),
                        crate::RunArg::Literal(format!(
                            "printf 'late' > \"{}\"",
                            later_output_path.display()
                        )),
                    ],
                    prompt: None,
                    path: None,
                    subject: None,
                    text: None,
                    agent: None,
                    input_overrides: None,
                    inputs: None,
                    input_mode: None,
                    platforms: None,
                },
            ],
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let error = apply_actions(
            &crate::Output { answer: 4 },
            &[first_action, second_action],
            &serde_json::Map::new(),
            &[],
            crate::ActionExecutionMode::Parallel,
            None,
            &provider_context(),
            5,
            runtime_budget,
        )
        .await
        .expect_err("abort should fail the parallel invocation");

        let later_output_exists = later_output_path.exists();
        let _ = fs::remove_file(&later_output_path);

        assert!(error.contains("Run aborted by [A1 first_action]"));
        assert!(error.contains("exit status: 19"));
        assert!(
            !later_output_exists,
            "other lanes should not start later steps after a peer lane aborts"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn apply_actions_continue_mode_exposes_failed_status_to_later_when() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2040-continue-child-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);
        let output_path = std::env::temp_dir().join(format!(
            "cai2040-continue-output-{}.txt",
            std::process::id()
        ));

        let script_body = format!("#!/bin/sh\nprintf 'ran' > \"{}\"\n", output_path.display());
        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let failing_step = crate::RunStep {
            kind: "exec".to_string(),
            program: Some("/bin/sh".to_string()),
            model: None,
            profile: None,
            output_variable: None,
            status_variable: Some("step_status".to_string()),
            error_variable: Some("step_error".to_string()),
            failure_mode: Some(crate::FailureMode::Continue),
            when: None,
            args: vec![
                crate::RunArg::Literal("-lc".to_string()),
                crate::RunArg::Literal("exit 9".to_string()),
            ],
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: None,
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };
        let second_step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            model: None,
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: Some(json!({ "==": [{ "var": "step_status" }, "failed"] })),
            args: Vec::new(),
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: Some(format!("./{}", script_name)),
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = apply_actions(
            &crate::Output { answer: 4 },
            &[action(vec![failing_step, second_step])],
            &serde_json::Map::new(),
            &[],
            crate::ActionExecutionMode::Sequential,
            None,
            &provider_context(),
            5,
            runtime_budget,
        )
        .await;

        let _ = fs::remove_file(&script_path);

        assert!(
            result.is_ok(),
            "continue-mode failure should not stop the action"
        );

        let file_contents =
            fs::read_to_string(&output_path).expect("later step should have executed");
        let _ = fs::remove_file(&output_path);

        assert_eq!(file_contents, "ran");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn apply_actions_allows_runtime_vars_in_action_logic() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2055-runtime-logic-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);
        let output_path =
            std::env::temp_dir().join(format!("cai2055-runtime-logic-{}.txt", std::process::id()));

        let script_body = format!("#!/bin/sh\nprintf 'ran' > \"{}\"\n", output_path.display());
        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            model: None,
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: Some(format!("./{}", script_name)),
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };
        let action = crate::Action {
            name: "runtime_gate".to_string(),
            logic: json!({ "==": [{ "var": "runtime.generate_images" }, true] }),
            run: vec![step],
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = apply_actions(
            &crate::Output { answer: 4 },
            &[action],
            &runtime_vars(&[("generate_images", json!(true))]),
            &[],
            crate::ActionExecutionMode::Sequential,
            None,
            &provider_context(),
            5,
            runtime_budget,
        )
        .await;

        let _ = fs::remove_file(&script_path);

        assert!(
            result.is_ok(),
            "runtime-gated action should succeed: {result:?}"
        );

        let file_contents = fs::read_to_string(&output_path).expect("action should have run");
        let _ = fs::remove_file(&output_path);
        assert_eq!(file_contents, "ran");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn apply_actions_allows_runtime_vars_in_step_when() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2055-runtime-when-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);
        let output_path =
            std::env::temp_dir().join(format!("cai2055-runtime-when-{}.txt", std::process::id()));

        let script_body = format!("#!/bin/sh\nprintf 'ran' > \"{}\"\n", output_path.display());
        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            model: None,
            profile: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: Some(json!({ "==": [{ "var": "runtime.generate_images" }, true] })),
            args: Vec::new(),
            prompt: None,
            path: None,
            subject: None,
            text: None,
            agent: Some(format!("./{}", script_name)),
            input_overrides: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = apply_actions(
            &crate::Output { answer: 4 },
            &[action(vec![step])],
            &runtime_vars(&[("generate_images", json!(true))]),
            &[],
            crate::ActionExecutionMode::Sequential,
            None,
            &provider_context(),
            5,
            runtime_budget,
        )
        .await;

        let _ = fs::remove_file(&script_path);

        assert!(
            result.is_ok(),
            "runtime-gated step should succeed: {result:?}"
        );

        let file_contents = fs::read_to_string(&output_path).expect("step should have run");
        let _ = fs::remove_file(&output_path);
        assert_eq!(file_contents, "ran");
    }

    #[test]
    fn validate_agent_action_depth_allows_nested_calls_below_limit() {
        let result = validate_agent_action_depth(2, 5, "invoke_child");

        assert!(result.is_ok(), "depth below limit should be allowed");
    }

    #[test]
    fn validate_agent_action_depth_rejects_when_limit_is_reached() {
        let error = validate_agent_action_depth(5, 5, "invoke_child")
            .expect_err("depth at limit should be rejected");

        assert!(error.contains("current depth 5"));
        assert!(error.contains("max-agent-depth 5"));
    }

    #[test]
    fn validate_agent_action_depth_rejects_zero_depth_limit() {
        let error = validate_agent_action_depth(0, 0, "invoke_child")
            .expect_err("zero max depth should disable child invocation");

        assert!(error.contains("current depth 0"));
        assert!(error.contains("max-agent-depth 0"));
    }

    #[test]
    fn formats_backend_ui_success_with_kind_prefix() {
        let response = json!({
            "ui": {
                "schema": "1.0",
                "kind": "success",
                "title": "Email sent",
                "summary": "Test email sent to sales@analyzer1.com.",
                "next_steps": ["Check your inbox and spam folder for the message."]
            }
        });

        let rendered =
            format_backend_ui_message(&response, true).expect("success ui should format");

        assert!(rendered.contains("✅ Email sent"));
        assert!(rendered.contains("Test email sent to sales@analyzer1.com."));
        assert!(rendered.contains("Next steps:"));
    }

    #[test]
    fn formats_backend_ui_failure_without_kind_prefix() {
        let response = json!({
            "ui": {
                "schema": "1.0",
                "kind": "failure",
                "title": "Request failed",
                "summary": "Email sending is disabled for this account.",
                "next_steps": ["Enable mail and retry."]
            }
        });

        let rendered = format_backend_error_message(&response).expect("failure ui should format");

        assert!(rendered.starts_with("Request failed"));
        assert!(!rendered.contains("❌ Request failed"));
        assert!(rendered.contains("Email sending is disabled for this account."));
        assert!(rendered.contains("Next steps:"));
    }
}
