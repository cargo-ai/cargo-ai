//! Native-only role selection. Private bootstrap state never enters business data.
use crate::execution_policy::{ExecutionPolicy, ModelSelection, RequestKind};
use crate::providers::thinking::ThinkingSetting;
use crate::role_contract::{BindingRevision, CallLocator, Resolution, ResolvedCall};
use crate::role_session::{Boundary, Permit, Session};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::sync::{Arc, Mutex};

tokio::task_local! { static CURRENT: Option<Context>; }
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bootstrap {
    pub version: u32,
    pub resolution: Resolution,
    pub bindings: BindingRevision,
    pub policy: ExecutionPolicy,
    pub definition: String,
    pub package_root: std::path::PathBuf,
    pub content_identities: std::collections::BTreeMap<String, String>,
    pub invocation_id: String,
    pub binding_revision: String,
    #[serde(default)]
    pub parent_permit_id: Option<String>,
}
#[derive(Clone)]
pub struct Context {
    pub bootstrap: Arc<Bootstrap>,
    pub locator: CallLocator,
    pub session: Session,
    pub parent_permit_id: Option<String>,
    result: Arc<Mutex<Option<Value>>>,
    first_error: Arc<Mutex<Option<String>>>,
}
impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoleContext")
            .field("locator", &self.locator)
            .finish_non_exhaustive()
    }
}
impl Context {
    pub fn new(bootstrap: Bootstrap, session: Session) -> Result<Self, String> {
        if session.is_closed() {
            return Err(failure("role.revoked"));
        }
        if bootstrap.version != 1
            || !bootstrap.resolution.ready
            || bootstrap.resolution.execution_authorized
            || bootstrap.bindings.revision != session.binding_revision
            || bootstrap.invocation_id != session.invocation_id
            || bootstrap.binding_revision != session.binding_revision
            || bootstrap
                .parent_permit_id
                .as_ref()
                .is_some_and(|id| id.is_empty() || id.len() > 128)
            || crate::role_contract::canonical_identity(&bootstrap.bindings)
                .map_err(|e| e.to_string())?
                != bootstrap.resolution.binding_identity
        {
            return Err(failure("role.invalid_bootstrap"));
        }
        if bootstrap
            .policy
            .allowed
            .iter()
            .any(|grant| !bootstrap.resolution.required_selections.contains(grant))
            || bootstrap.policy.limits.max_runtime_secs
                > bootstrap.resolution.scope.limits.max_runtime_secs
            || bootstrap.policy.limits.max_output_tokens
                > bootstrap.resolution.scope.limits.max_output_tokens
            || bootstrap.policy.limits.max_agent_depth
                > bootstrap.resolution.scope.limits.max_agent_depth
        {
            return Err(failure("role.policy_scope_conflict"));
        }
        let context = Self {
            parent_permit_id: bootstrap.parent_permit_id.clone(),
            locator: CallLocator {
                definition: bootstrap.definition.clone(),
                site: "root".into(),
            },
            bootstrap: Arc::new(bootstrap),
            session,
            result: Arc::new(Mutex::new(None)),
            first_error: Arc::new(Mutex::new(None)),
        };
        context.validate_context()?;
        if context
            .bootstrap
            .resolution
            .calls
            .iter()
            .any(|call| call.call_site.locator == context.locator)
        {
            context.selected()?;
        }
        Ok(context)
    }
    pub async fn scope<T>(&self, future: impl Future<Output = T>) -> T {
        CURRENT
            .scope(Some(self.clone()), self.bootstrap.policy.scope(future))
            .await
    }
    pub fn at(&self, site: String) -> Self {
        Self {
            locator: CallLocator {
                definition: self.locator.definition.clone(),
                site,
            },
            ..self.clone()
        }
    }
    pub fn validate_context(&self) -> Result<(), String> {
        self.validated_fixed_connections().map(|_| ())
    }
    fn validated_fixed_connections(
        &self,
    ) -> Result<
        std::collections::BTreeMap<
            String,
            crate::credentials::role_context::ValidatedProfileContext,
        >,
        String,
    > {
        CURRENT.sync_scope(Some(self.clone()), || {
            if self.session.is_closed() {
                return Err(failure("role.revoked"));
            }
            verify_contents(
                &self.bootstrap.package_root,
                &self.bootstrap.content_identities,
            )?;
            // Structural coordinators and tool descriptions must also stop when any
            // connection in the reviewed private revision changes.
            for binding in &self.bootstrap.bindings.bindings {
                let selected = crate::credentials::role_context::resolve_profile_context(
                    &binding.profile_uuid,
                    &binding.connection_generation,
                )
                .map_err(|_| failure("role.stale_connection"))?;
                if selected.profile_name != binding.profile
                    || !selected
                        .profile
                        .server
                        .eq_ignore_ascii_case(&binding.provider)
                {
                    return Err(failure("role.stale_connection"));
                }
            }
            let fixed: Vec<_> = self
                .bootstrap
                .resolution
                .calls
                .iter()
                .filter_map(|call| {
                    call.call_site
                        .fixed
                        .as_ref()
                        .map(|selection| (call.call_site.id.clone(), selection.profile.clone()))
                })
                .collect();
            let home = crate::credentials::role_context::root()
                .map_err(|_| failure("role.stale_connection"))?;
            let (fixed_identity, snapshots) =
                crate::providers::operation_metadata::fixed_context_snapshot(&home, &fixed)
                    .map_err(|_| failure("role.stale_connection"))?;
            let identity = crate::providers::operation_metadata::connection_context_identity(
                &self.bootstrap.bindings,
                &fixed_identity,
            )
            .map_err(|_| failure("role.stale_connection"))?;
            if identity
                != self
                    .bootstrap
                    .resolution
                    .context
                    .connection_context_identity
            {
                return Err(failure("role.stale_connection"));
            }
            Ok(snapshots)
        })
    }
    pub fn selected(&self) -> Result<ResolvedCall, String> {
        CURRENT.sync_scope(Some(self.clone()), || {
            self.validate_context()?;
            let call = self
                .bootstrap
                .resolution
                .calls
                .iter()
                .find(|c| c.call_site.locator == self.locator)
                .ok_or_else(|| failure("role.undeclared_call_site"))?
                .clone();
            if call.selection.is_none() {
                let structural = matches!(
                    call.call_site.kind,
                    crate::role_contract::CallKind::Child
                        | crate::role_contract::CallKind::ToolChild
                ) && call.call_site.role.is_none()
                    && call.call_site.fixed.is_none()
                    && call.call_site.target.as_ref().is_some_and(|target| {
                        !self.bootstrap.resolution.calls.iter().any(|child| {
                            child.call_site.locator.definition == *target
                                && child.call_site.locator.site == "root"
                        })
                    });
                if structural {
                    return Ok(call);
                }
                return Err(failure("role.unresolved"));
            }
            let selection = call.selection.as_ref().expect("selection checked");
            if let Some(role) = &call.call_site.role {
                let binding = self
                    .bootstrap
                    .bindings
                    .bindings
                    .iter()
                    .find(|b| &b.role == role)
                    .ok_or_else(|| failure("role.unresolved"))?;
                let private = crate::credentials::role_context::resolve_profile_context(
                    &binding.profile_uuid,
                    &binding.connection_generation,
                )
                .map_err(|_| failure("role.stale_connection"))?;
                if private.profile_name != binding.profile
                    || !private
                        .profile
                        .server
                        .eq_ignore_ascii_case(&binding.provider)
                {
                    return Err(failure("role.stale_connection"));
                }
            }
            if !self.bootstrap.policy.allowed.contains(selection) {
                return Err(failure("action.execution_selection_denied"));
            }
            Ok(call)
        })
    }
    pub fn boundary(&self, kind: &str) -> Result<Boundary, String> {
        let call = self.selected()?;
        Ok(Boundary {
            invocation_id: self.session.invocation_id.clone(),
            binding_revision: self.session.binding_revision.clone(),
            parent_permit_id: self.parent_permit_id.clone(),
            call_site: call.call_site.id,
            agent: self.locator.definition.clone(),
            target_agent: call.call_site.target,
            kind: kind.into(),
        })
    }
    pub fn child_bootstrap(&self, target: &str) -> Result<Bootstrap, String> {
        CURRENT.sync_scope(Some(self.clone()), || {
            let call = self.selected()?;
            if call.call_site.target.as_deref() != Some(target) {
                return Err(failure("role.child_target_conflict"));
            }
            let mut bootstrap = (*self.bootstrap).clone();
            bootstrap.definition = target.to_owned();
            let child = self.bootstrap.resolution.calls.iter().find(|c| {
                c.call_site.locator.definition == target && c.call_site.locator.site == "root"
            });
            if child.and_then(|c| c.selection.as_ref()) != call.selection.as_ref() {
                return Err(failure("role.child_selection_conflict"));
            }
            Ok(bootstrap)
        })
    }
}
pub fn verify_contents(
    root: &std::path::Path,
    expected: &std::collections::BTreeMap<String, String>,
) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    use std::path::Component;
    if expected.is_empty() || expected.len() > 2048 {
        return Err(failure("role.package_identity_unavailable"));
    }
    let mut total = 0u64;
    for (relative, digest) in expected {
        let candidate = std::path::Path::new(relative);
        let mut path = if candidate.is_absolute() {
            std::path::PathBuf::new()
        } else {
            root.to_path_buf()
        };
        for part in candidate.components() {
            match part {
                Component::Normal(value) => path.push(value),
                Component::RootDir | Component::Prefix(_) => {
                    path.push(part.as_os_str());
                    continue;
                }
                _ => return Err(failure("role.package_changed")),
            };
            let metadata =
                std::fs::symlink_metadata(&path).map_err(|_| failure("role.package_changed"))?;
            if metadata.file_type().is_symlink() {
                return Err(failure("role.package_changed"));
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(failure("role.package_changed"));
                }
            }
        }
        let mut file = std::fs::File::open(&path).map_err(|_| failure("role.package_changed"))?;
        let metadata = file
            .metadata()
            .map_err(|_| failure("role.package_changed"))?;
        total = total
            .checked_add(metadata.len())
            .ok_or_else(|| failure("role.package_changed"))?;
        if !metadata.is_file() || total > 512 * 1024 * 1024 {
            return Err(failure("role.package_changed"));
        }
        let mut hasher = Sha256::new();
        let mut buffer = [0; 65536];
        let mut observed = 0u64;
        loop {
            let count = file
                .read(&mut buffer)
                .map_err(|_| failure("role.package_changed"))?;
            if count == 0 {
                break;
            }
            observed += count as u64;
            if observed > metadata.len() {
                return Err(failure("role.package_changed"));
            }
            hasher.update(&buffer[..count]);
        }
        if format!("{:x}", hasher.finalize()) != *digest {
            return Err(failure("role.package_changed"));
        }
    }
    Ok(())
}
pub fn current() -> Option<Context> {
    CURRENT.try_with(Clone::clone).ok().flatten()
}
pub async fn scope<T>(context: Option<Context>, future: impl Future<Output = T>) -> T {
    match context {
        Some(context) => context.scope(future).await,
        None => CURRENT.scope(None, future).await,
    }
}
pub async fn scope_step<T>(site: String, future: impl Future<Output = T>) -> T {
    match current() {
        Some(context) => context.at(site).scope(future).await,
        None => future.await,
    }
}
pub fn selected() -> Result<Option<ResolvedCall>, String> {
    current().map(|c| c.selected()).transpose()
}
pub fn capture_business_result(value: &Value) {
    if let Some(context) = current() {
        if serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() < 128 * 1024) {
            *context.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(value.clone());
        } else {
            *context
                .first_error
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some("role.result_limit".into());
        }
    }
}
pub fn note_error(code: &str) {
    if let Some(context) = current() {
        context
            .first_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(|| code.to_owned());
    }
}
pub fn finish_native(succeeded: bool) -> Result<(), String> {
    let context = current().ok_or("role.context_unavailable")?;
    let error = context
        .first_error
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let snapshot = context.session.snapshot();
    let failed = error.is_some() || !succeeded;
    let frame = serde_json::json!({"protocol":"cargo_ai_native_role","version":1,"type":"result",
        "invocation_id":context.session.invocation_id,"binding_revision":context.session.binding_revision,
        "result":context.result.lock().unwrap_or_else(|e|e.into_inner()).clone(),
        "error":if failed {serde_json::json!({"code":error.unwrap_or_else(||"runtime.failed".into()),"completion":"failed","effects":"may_have_been_applied"})}else{Value::Null},
        "native_outcomes":snapshot});
    crate::role_session::write_frame(&mut std::io::stdout().lock(), &frame)
        .map_err(|_| "role.control_lost".to_owned())
}
pub fn failure(code: &'static str) -> String {
    note_error(code);
    if let Some(context) = current() {
        let call = context
            .bootstrap
            .resolution
            .calls
            .iter()
            .find(|c| c.call_site.locator == context.locator);
        let call_site = call
            .map(|c| c.call_site.id.clone())
            .unwrap_or_else(|| context.locator.site.clone());
        context.session.record_denial(
            Boundary {
                invocation_id: context.session.invocation_id.clone(),
                binding_revision: context.session.binding_revision.clone(),
                parent_permit_id: context.parent_permit_id.clone(),
                call_site,
                agent: context.locator.definition.clone(),
                target_agent: call.and_then(|c| c.call_site.target.clone()),
                kind: if call.is_some_and(|c| {
                    matches!(
                        c.call_site.kind,
                        crate::role_contract::CallKind::Child
                            | crate::role_contract::CallKind::ToolChild
                    )
                }) {
                    "native_child"
                } else {
                    "provider"
                }
                .into(),
            },
            code,
        );
    }
    #[cfg(cargo_ai_cli)]
    crate::commands::machine::record_error(crate::commands::machine::Failure::new(
        code,
        "Native role execution could not satisfy its declared context and authorization.",
    ));
    code.to_owned()
}
/// Role-bearing selectors are owned by the validated binding. Equal explicit
/// settings are harmless, but a conflicting override is never silently ignored.
pub fn root_selection(
    profile: Option<&str>,
    model: Option<&str>,
    thinking: Option<&ThinkingSetting>,
    inference_required: bool,
) -> Result<Option<ResolvedCall>, String> {
    if !inference_required {
        if let Some(context) = current() {
            if context
                .bootstrap
                .resolution
                .calls
                .iter()
                .any(|call| call.call_site.locator == context.locator)
            {
                return Err(failure("role.root_selection_conflict"));
            }
            return Ok(None);
        }
    }
    let Some(call) = selected()? else {
        return Ok(None);
    };
    let selection = call
        .selection
        .as_ref()
        .ok_or_else(|| failure("role.unresolved"))?;
    let exact_model = match &selection.model {
        ModelSelection::Named { value } => Some(value.as_str()),
        _ => None,
    };
    if profile.is_some_and(|v| Some(v) != selection.profile.as_deref())
        || model.is_some_and(|v| Some(v) != exact_model)
        || thinking.is_some_and(|v| v != &selection.thinking)
    {
        return Err(failure("role.selector_conflict"));
    }
    Ok(Some(call))
}
pub fn apply_step(step: &crate::RunStep) -> Result<crate::RunStep, String> {
    if current().is_none() {
        return Ok(step.clone());
    }
    if !matches!(
        step.kind.as_str(),
        "agent" | "generate_image" | "generate_audio" | "transcribe_audio"
    ) {
        return Ok(step.clone());
    }
    let call = selected()?.ok_or_else(|| failure("role.unresolved"))?;
    // Fixed sites retain source configuration. Portable validation already binds
    // the fixed selection; provider dispatch compares the effective request again.
    if call.call_site.role.is_none() {
        return Ok(step.clone());
    }
    if step.profile.is_some() || step.model.is_some() || step.thinking.is_some() {
        return Err(failure("role.selector_conflict"));
    }
    if let Some(voice) = &step.voice {
        if !matches!(voice,crate::RunArg::Literal(value) if call.settings.get("voice").and_then(Value::as_str)==Some(value.as_str()))
        {
            return Err(failure("role.selector_conflict"));
        }
    }
    let selection = call
        .selection
        .as_ref()
        .ok_or_else(|| failure("role.unresolved"))?;
    let mut step = step.clone();
    step.profile = selection.profile.clone().map(crate::RunArg::Literal);
    step.model = match &selection.model {
        ModelSelection::Named { value } => Some(crate::RunArg::Literal(value.clone())),
        _ => None,
    };
    step.thinking = Some(match &selection.thinking {
        ThinkingSetting::ProviderDefault => ThinkingSetting::ProviderDefault,
        ThinkingSetting::On => ThinkingSetting::On,
        ThinkingSetting::Off => ThinkingSetting::Off,
        ThinkingSetting::Choice { value } => ThinkingSetting::Choice {
            value: crate::RunArg::Literal(value.clone()),
        },
    });
    step.voice = call
        .settings
        .get("voice")
        .and_then(Value::as_str)
        .map(|s| crate::RunArg::Literal(s.to_owned()));
    Ok(step)
}
pub async fn admit_provider(
    provider: crate::providers::ProviderKind,
    url: &str,
    token: &str,
    account_id: Option<&str>,
    model: &str,
    kind: RequestKind,
    _modalities: &[&str],
    _structured: bool,
    settings: serde_json::Value,
) -> Result<Option<Permit>, String> {
    let Some(context) = current() else {
        return Ok(None);
    };
    let call = context.selected()?;
    let selection = call
        .selection
        .as_ref()
        .ok_or_else(|| failure("role.unresolved"))?;
    let effective_model = if model.is_empty()
        && kind == RequestKind::Audio
        && provider == crate::providers::ProviderKind::Xai
    {
        ModelSelection::FixedService {}
    } else {
        ModelSelection::named_or_default(model)
    };
    if selection.request_kind != kind || selection.model != effective_model {
        return Err(failure("role.request_conflict"));
    }
    if let Some(role) = &call.call_site.role {
        let binding = context
            .bootstrap
            .bindings
            .bindings
            .iter()
            .find(|b| &b.role == role)
            .ok_or_else(|| failure("role.unresolved"))?;
        if crate::providers::ProviderKind::from_server_value(&binding.provider) != Some(provider) {
            return Err(failure("role.request_conflict"));
        }
    }
    let expected_thinking = match (&selection.thinking, provider, kind) {
        (ThinkingSetting::On, crate::providers::ProviderKind::Ollama, RequestKind::Text) => {
            serde_json::json!({"mode":"choice","value":"medium"})
        }
        (ThinkingSetting::Off, crate::providers::ProviderKind::Ollama, RequestKind::Text) => {
            serde_json::json!({"mode":"choice","value":"none"})
        }
        _ => serde_json::to_value(&selection.thinking)
            .map_err(|_| failure("role.request_conflict"))?,
    };
    if settings
        .get("thinking")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({"mode":"provider_default"}))
        != expected_thinking
    {
        return Err(failure("role.request_conflict"));
    }
    for key in ["voice", "format", "temperature"] {
        if call.settings.get(key) != settings.get(key) {
            return Err(failure("role.request_conflict"));
        }
    }
    // Match the bytes actually held by the request to the same validated snapshot
    // whose reference participates in this invocation's reviewed connection identity.
    let private = if let Some(role) = &call.call_site.role {
        let binding = context
            .bootstrap
            .bindings
            .bindings
            .iter()
            .find(|binding| &binding.role == role)
            .ok_or_else(|| failure("role.unresolved"))?;
        let private = crate::credentials::role_context::resolve_profile_context(
            &binding.profile_uuid,
            &binding.connection_generation,
        )
        .map_err(|_| failure("role.stale_connection"))?;
        if private.profile_name != binding.profile {
            return Err(failure("role.stale_connection"));
        }
        private
    } else {
        context
            .validated_fixed_connections()?
            .remove(&call.call_site.id)
            .ok_or_else(|| failure("role.unresolved"))?
    };
    let default_url = if private.profile.auth_mode
        == crate::config::schema::ProfileAuthMode::OpenaiAccount
        && provider == crate::providers::ProviderKind::OpenAi
    {
        "https://chatgpt.com/backend-api/codex/responses"
    } else {
        provider.default_url()
    };
    let expected_url = private
        .profile
        .url
        .as_deref()
        .filter(|url| !url.is_empty())
        .unwrap_or(default_url);
    if crate::providers::ProviderKind::from_server_value(&private.profile.server) != Some(provider)
        || url != expected_url
        || !private.matches(token, account_id)
    {
        return Err(failure("role.stale_connection"));
    }
    context
        .session
        .admit(Boundary {
            invocation_id: context.session.invocation_id.clone(),
            binding_revision: context.session.binding_revision.clone(),
            parent_permit_id: context.parent_permit_id.clone(),
            call_site: call.call_site.id,
            agent: context.locator.definition.clone(),
            target_agent: call.call_site.target,
            kind: "provider".into(),
        })
        .await
        .map(Some)
        .map_err(|_| failure("role.revoked"))
}
pub fn provider_settings(thinking: Option<&str>, temperature: Option<f64>) -> serde_json::Value {
    let mut settings = serde_json::json!({"thinking":match thinking {Some(value)=>serde_json::json!({"mode":"choice","value":value}),None=>serde_json::json!({"mode":"provider_default"})}});
    if let Some(value) = temperature {
        settings["temperature"] = serde_json::json!(value)
    }
    settings
}
pub fn settle_provider<T>(
    permit: Option<Permit>,
    result: &Result<T, crate::providers::ProviderError>,
) {
    if let Some(permit) = permit {
        match result {
            Ok(_) => permit.finish("completed", None),
            Err(error) => {
                use crate::providers::ProviderErrorKind::*;
                let cause = match error.kind() {
                    Unauthorized => "provider.authentication",
                    ModelNotFound => "provider.model_unavailable",
                    RateLimited => "provider.rate_limited",
                    Timeout => "provider.timeout",
                    Connectivity => "provider.connectivity",
                    InvalidRequest => "provider.invalid_request",
                    InvalidResponse => "provider.invalid_response",
                    Unknown => "provider.unknown",
                };
                permit.finish(
                    if matches!(error.kind(), Timeout | Connectivity | Unknown) {
                        "completion_unknown"
                    } else {
                        "failed"
                    },
                    Some(cause),
                );
            }
        }
    }
}

#[cfg(all(test, cargo_ai_cli))]
mod tests {
    use super::*;
    use crate::execution_policy::{AllowedSelection, ExecutionLimits};
    use crate::role_contract::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    struct HomeGuard(Option<std::ffi::OsString>);
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.0 {
                std::env::set_var("CARGO_AI_HOME", value);
            } else {
                std::env::remove_var("CARGO_AI_HOME");
            }
        }
    }
    fn fixture() -> (Context, std::path::PathBuf, HomeGuard) {
        fixture_at("http://127.0.0.1:1")
    }
    fn fixture_at(url: &str) -> (Context, std::path::PathBuf, HomeGuard) {
        fixture_at_provider(url, "openai")
    }
    fn fixture_at_provider(url: &str, provider: &str) -> (Context, std::path::PathBuf, HomeGuard) {
        let root =
            std::env::temp_dir().join(format!("cargo-ai-role-runtime-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let home = root.join("home");
        std::fs::create_dir(&home).unwrap();
        let guard = HomeGuard(std::env::var_os("CARGO_AI_HOME"));
        std::env::set_var("CARGO_AI_HOME", &home);
        std::fs::write(home.join("config.toml"), format!("secret_store='file'\ndefault_profile='fixture'\n[[profile]]\nname='fixture'\nserver='{provider}'\nmodel='fixture'\nauth_mode='api_key'\nurl='{url}'\n")).unwrap();
        std::fs::write(
            home.join("credentials.toml"),
            "[profile_tokens]\nfixture='isolated-fixture'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::set_permissions(
                home.join("credentials.toml"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        crate::credentials::role_context::refresh_profile_context("fixture").unwrap();
        let fixed_identity = crate::providers::operation_metadata::fixed_context_identity(
            &home,
            &[("root".into(), None)],
        )
        .unwrap();
        let file = root.join("agent.json");
        std::fs::write(&file, b"verified-source").unwrap();
        use sha2::{Digest, Sha256};
        let limits = ExecutionLimits {
            max_runtime_secs: 60,
            max_output_tokens: 512,
            max_agent_depth: 4,
        };
        let selection = AllowedSelection {
            profile: None,
            request_kind: RequestKind::Text,
            model: ModelSelection::Named {
                value: "fixture".into(),
            },
            thinking: ThinkingSetting::ProviderDefault,
        };
        let bindings = BindingRevision {
            version: 1,
            revision: "revision-a".into(),
            bindings: vec![],
        };
        let call_site = CallSite {
            id: "root".into(),
            locator: CallLocator {
                definition: "agent.json".into(),
                site: "root".into(),
            },
            kind: CallKind::Root,
            role: None,
            fixed: Some(FixedSelection {
                profile: None,
                model: selection.model.clone(),
                settings: BTreeMap::new(),
            }),
            requirements: OperationRequirements {
                operation: "text_generation".into(),
                input_modalities: vec!["text".into()],
                structured_output: true,
                settings: BTreeMap::new(),
            },
            target: None,
            artifact: None,
            input_schema: None,
        };
        let resolution = Resolution {
            version: 1,
            identity: "test-only-native-resolution".into(),
            binding_revision: bindings.revision.clone(),
            binding_identity: canonical_identity(&bindings).unwrap(),
            context: ResolutionContext {
                project_identity: "fixture".into(),
                environment_identity: "fixture".into(),
                package_identity: "fixture".into(),
                runtime_contract: "fixture".into(),
                connection_context_identity:
                    crate::providers::operation_metadata::connection_context_identity(
                        &bindings,
                        &fixed_identity,
                    )
                    .unwrap(),
                consent_identity: "fixture".into(),
            },
            scope: RoleContext {
                key: ContextKey {
                    action: "draw".into(),
                    interface: "native".into(),
                    mode: "default".into(),
                },
                call_sites: vec!["root".into()],
                resources: vec![],
                data_scopes: vec![],
                limits: limits.clone(),
            },
            calls: vec![ResolvedCall {
                call_site,
                selection: Some(selection.clone()),
                settings: BTreeMap::new(),
                compatibility: Compatibility::Unknown,
                reason: "fixed selection disclosed".into(),
                evidence: vec![],
            }],
            required_selections: vec![selection.clone()],
            ready: true,
            execution_authorized: false,
            invocation_access: "unverified".into(),
        };
        let session = Session::new(bindings.revision.clone());
        let bootstrap = Bootstrap {
            version: 1,
            resolution,
            bindings,
            policy: ExecutionPolicy {
                version: 1,
                allowed: vec![selection],
                limits,
            },
            definition: "agent.json".into(),
            package_root: root.clone(),
            content_identities: BTreeMap::from([(
                file.to_string_lossy().into_owned(),
                format!("{:x}", Sha256::digest(b"verified-source")),
            )]),
            invocation_id: session.invocation_id.clone(),
            binding_revision: session.binding_revision.clone(),
            parent_permit_id: None,
        };
        (Context::new(bootstrap, session).unwrap(), root, guard)
    }
    async fn request(
        url: &str,
        model: &str,
    ) -> Result<crate::providers::runtime::ProviderTextResponse, crate::providers::ProviderError>
    {
        request_with_credential(url, model, "isolated-fixture", None).await
    }
    async fn request_with_credential(
        url: &str,
        model: &str,
        token: &str,
        account_id: Option<&str>,
    ) -> Result<crate::providers::runtime::ProviderTextResponse, crate::providers::ProviderError>
    {
        crate::providers::send_text_request_with_account_context(crate::providers::ProviderKind::OpenAi,url,
            crate::providers::ProviderTextRequest{rubric_enabled:false,model,content_parts:&[crate::providers::runtime::ContentPart::Text("business input".into())],timeout_in_sec:5,token,response_schema:&json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false}),max_output_tokens:Some(128),temperature:None,thinking:None},account_id).await
    }
    #[tokio::test]
    async fn native_role_initial_denial_keeps_root_context_before_async_scope() {
        let (context, root, _home_guard) = fixture();
        let mut bootstrap = (*context.bootstrap).clone();
        bootstrap.policy.allowed.clear();
        assert!(current().is_none());
        assert!(Context::new(bootstrap, context.session.clone()).is_err());
        let snapshot = context.session.snapshot();
        let denial = snapshot["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["cause"] == "action.execution_selection_denied")
            .unwrap();
        assert_eq!(denial["boundary"]["agent"], "agent.json");
        assert_eq!(denial["boundary"]["call_site"], "root");
        assert!(denial["boundary"]["parent_permit_id"].is_null());
        assert!(denial["boundary"]["target_agent"].is_null());
        assert_eq!(denial["state"], "not_dispatched");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn revoked_runtime_preserves_admitted_result_and_blocks_later_http() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let (context, root, _home_guard) = fixture_at(&url);
        let running_context = context.clone();
        let first_url = url.clone();
        let running =
            tokio::spawn(
                async move { running_context.scope(request(&first_url, "fixture")).await },
            );
        let (mut socket, _) =
            tokio::time::timeout(std::time::Duration::from_secs(3), listener.accept())
                .await
                .unwrap()
                .unwrap();
        let mut input = [0u8; 4096];
        assert!(socket.read(&mut input).await.unwrap() > 0);
        let ack = context.session.revoke("revision-a").unwrap();
        assert_eq!(ack["outstanding"].as_object().unwrap().len(), 1);
        assert!(context.scope(request(&url, "fixture")).await.is_err());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(40), listener.accept())
                .await
                .is_err()
        );
        let body = r#"{"id":"fixture","object":"chat.completion","created":0,"model":"fixture","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"{\"ok\":true}"}}]}"#;
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).as_bytes()).await.unwrap();
        drop(socket);
        let response = running.await.unwrap();
        assert!(response.is_ok(), "{response:?}");
        let snapshot = context.session.snapshot();
        assert!(snapshot["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["state"] == "completed"));
        assert!(snapshot["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["cause"] == "role.revoked"));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn native_signed_account_renewal_dispatches_only_the_validated_snapshot() {
        use crate::credentials::access_continuity::test_support;
        const ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
        const ACCOUNT: &str = "synthetic-account";
        struct AccountGuard(Option<std::ffi::OsString>);
        impl Drop for AccountGuard {
            fn drop(&mut self) {
                test_support::set_keys(None);
                if let Some(value) = &self.0 {
                    std::env::set_var("CODEX_HOME", value);
                } else {
                    std::env::remove_var("CODEX_HOME");
                }
            }
        }
        let (initial, root, _home_guard) = fixture();
        let _account_guard = AccountGuard(std::env::var_os("CODEX_HOME"));
        let codex_home = root.join("codex");
        std::env::set_var("CODEX_HOME", &codex_home);
        let now = crate::credentials::role_context::now();
        let mut claims = test_support::claims(now);
        let original_token = test_support::write_session(&codex_home, &claims);
        test_support::set_keys(Some(Ok(test_support::keys(now))));
        let home = root.join("home");
        std::fs::write(home.join("config.toml"),
            "secret_store='file'\ndefault_profile='fixture'\n[[profile]]\nname='fixture'\nserver='openai'\nmodel='fixture'\nauth_mode='openai_account'\n").unwrap();
        let reference = crate::credentials::role_context::refresh_at(&home, "fixture").unwrap();
        let mut bootstrap = (*initial.bootstrap).clone();
        let fixed = crate::providers::operation_metadata::fixed_context_identity(
            &home,
            &[("root".into(), None)],
        )
        .unwrap();
        bootstrap.resolution.context.connection_context_identity =
            crate::providers::operation_metadata::connection_context_identity(
                &bootstrap.bindings,
                &fixed,
            )
            .unwrap();
        let context = Context::new(bootstrap, initial.session.clone()).unwrap();
        let role_context = bound_role_context(&context);
        let second_root = root.join("second-package");
        std::fs::create_dir(&second_root).unwrap();
        let second_source = second_root.join("agent.json");
        std::fs::write(&second_source, b"second-package-source").unwrap();
        let mut second_bootstrap = (*role_context.bootstrap).clone();
        second_bootstrap.package_root = second_root;
        use sha2::{Digest, Sha256};
        second_bootstrap.content_identities = BTreeMap::from([(
            second_source.to_string_lossy().into_owned(),
            format!("{:x}", Sha256::digest(b"second-package-source")),
        )]);
        second_bootstrap.resolution.context.package_identity = "second-package".into();
        second_bootstrap.resolution.context.project_identity = "second-project".into();
        second_bootstrap.resolution.context.consent_identity = "second-consent".into();
        second_bootstrap.resolution.scope.key.action = "second-action".into();
        second_bootstrap.bindings.revision = "revision-b".into();
        second_bootstrap.resolution.binding_revision = "revision-b".into();
        second_bootstrap.resolution.binding_identity =
            canonical_identity(&second_bootstrap.bindings).unwrap();
        let second_session = Session::new("revision-b".into());
        second_bootstrap.invocation_id = second_session.invocation_id.clone();
        second_bootstrap.binding_revision = second_session.binding_revision.clone();
        let second_context = Context::new(second_bootstrap, second_session).unwrap();
        assert_eq!(
            role_context.bootstrap.bindings.bindings[0].profile_uuid,
            second_context.bootstrap.bindings.bindings[0].profile_uuid
        );
        assert_eq!(
            role_context.bootstrap.bindings.bindings[0].connection_generation,
            second_context.bootstrap.bindings.bindings[0].connection_generation
        );
        let second_validation = second_context
            .scope(async {
                crate::credentials::role_context::validate_profile_context(
                    "fixture", &reference, false,
                )
                .unwrap()
            })
            .await;
        assert_eq!(second_validation.status, "unchanged");
        assert!(second_validation.execution_ready);
        role_context.validate_context().unwrap();
        let mut server = mockito::Server::new_async().await;
        let _transport =
            crate::providers::native_account_test_endpoint(format!("{}/native", server.url()));
        let reply =
            "data: {\"type\":\"response.output_text.done\",\"text\":\"{\\\"ok\\\":true}\"}\n\n";
        let original = server
            .mock("POST", "/native")
            .match_header("authorization", format!("Bearer {original_token}").as_str())
            .match_header("chatgpt-account-id", ACCOUNT)
            .with_header("content-type", "text/event-stream")
            .with_status(200)
            .with_body(reply)
            .expect(1)
            .create_async()
            .await;
        assert!(context
            .scope(request_with_credential(
                ENDPOINT,
                "fixture",
                &original_token,
                Some(ACCOUNT)
            ))
            .await
            .is_ok());
        original.assert_async().await;
        claims["exp"] = json!(now + 7200);
        claims["iat"] = json!(now - 30);
        claims["jti"] = json!("renewed-session-token");
        let renewed_token = test_support::write_session(&codex_home, &claims);
        assert_ne!(original_token, renewed_token);
        assert_eq!(
            second_context
                .scope(async {
                    crate::credentials::role_context::refresh_at(&home, "fixture").unwrap()
                })
                .await,
            reference
        );
        context.validate_context().unwrap();
        role_context.validate_context().unwrap();
        second_context.validate_context().unwrap();
        assert!(context
            .scope(request_with_credential(
                ENDPOINT,
                "fixture",
                &original_token,
                Some(ACCOUNT)
            ))
            .await
            .is_err());
        assert!(role_context
            .scope(request_with_credential(
                ENDPOINT,
                "fixture",
                &renewed_token,
                Some("different-account")
            ))
            .await
            .is_err());
        let renewed = server
            .mock("POST", "/native")
            .match_header("authorization", format!("Bearer {renewed_token}").as_str())
            .match_header("chatgpt-account-id", ACCOUNT)
            .with_header("content-type", "text/event-stream")
            .with_status(200)
            .with_body(reply)
            .expect(2)
            .create_async()
            .await;
        assert!(role_context
            .scope(request_with_credential(
                ENDPOINT,
                "fixture",
                &renewed_token,
                Some(ACCOUNT)
            ))
            .await
            .is_ok());
        assert!(second_context
            .scope(request_with_credential(
                ENDPOINT,
                "fixture",
                &renewed_token,
                Some(ACCOUNT)
            ))
            .await
            .is_ok());
        renewed.assert_async().await;
        original.assert_async().await;
        let snapshot = context.session.snapshot();
        let outcomes = snapshot["outcomes"].as_array().unwrap();
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| outcome["state"] == "completed")
                .count(),
            2
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| outcome["cause"] == "role.stale_connection"
                    && outcome["state"] == "not_dispatched")
                .count(),
            2
        );
        assert!(snapshot["outstanding"].as_object().unwrap().is_empty());
        let second_snapshot = second_context.session.snapshot();
        let second_outcomes = second_snapshot["outcomes"].as_array().unwrap();
        assert_eq!(second_outcomes.len(), 1);
        assert_eq!(second_outcomes[0]["state"], "completed");
        assert_eq!(
            second_outcomes[0]["boundary"]["binding_revision"],
            "revision-b"
        );
        assert!(second_snapshot["outstanding"]
            .as_object()
            .unwrap()
            .is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn native_provider_rejects_mismatched_credentials_account_and_endpoint() {
        let (context, root, _home_guard) = fixture();
        let role_context = bound_role_context(&context);
        for selected_context in [&context, &role_context] {
            for (url, token, account) in [
                ("http://127.0.0.1:1", "other-token", None),
                (
                    "http://127.0.0.1:1",
                    "isolated-fixture",
                    Some("other-account"),
                ),
                ("http://127.0.0.1:2", "isolated-fixture", None),
            ] {
                let error = selected_context
                    .scope(request_with_credential(url, "fixture", token, account))
                    .await
                    .unwrap_err();
                assert_eq!(
                    error.kind(),
                    crate::providers::ProviderErrorKind::InvalidRequest
                );
            }
        }
        let snapshot = context.session.snapshot();
        let outcomes = snapshot["outcomes"].as_array().unwrap();
        assert_eq!(outcomes.len(), 6);
        assert!(outcomes
            .iter()
            .all(|outcome| outcome["cause"] == "role.stale_connection"
                && outcome["state"] == "not_dispatched"));
        assert!(snapshot["outstanding"].as_object().unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn bound_role_context(context: &Context) -> Context {
        let private = crate::credentials::role_context::resolve_named_at(
            &context.bootstrap.package_root.join("home"),
            "fixture",
        )
        .unwrap();
        let mut bootstrap = (*context.bootstrap).clone();
        bootstrap.bindings.bindings.push(RoleBinding {
            role: "answer".into(),
            profile: "fixture".into(),
            profile_uuid: private.context.profile_uuid.clone(),
            connection_generation: private.context.connection_generation.clone(),
            provider: private.profile.server.clone(),
            model: "fixture".into(),
            settings: BTreeMap::new(),
        });
        let call = &mut bootstrap.resolution.calls[0];
        call.call_site.role = Some("answer".into());
        call.call_site.fixed = None;
        call.selection.as_mut().unwrap().profile = Some("fixture".into());
        call.evidence.push(CapabilityEvidence {
            profile_uuid: private.context.profile_uuid,
            connection_generation: private.context.connection_generation,
            provider: private.profile.server.clone(),
            model: "fixture".into(),
            operation: "text_generation".into(),
            input_modalities: vec!["text".into()],
            structured_output: true,
            settings: BTreeMap::new(),
            status: Compatibility::Compatible,
            evidence_revision: "fixture".into(),
            provenance: vec!["isolated fixture".into()],
            valid_until_unix_secs: u64::MAX,
            operation_evidence: None,
        });
        bootstrap.policy.allowed = vec![call.selection.clone().unwrap()];
        bootstrap.resolution.required_selections = bootstrap.policy.allowed.clone();
        bootstrap.resolution.binding_identity = canonical_identity(&bootstrap.bindings).unwrap();
        let fixed = crate::providers::operation_metadata::fixed_context_identity(
            &context.bootstrap.package_root.join("home"),
            &[],
        )
        .unwrap();
        bootstrap.resolution.context.connection_context_identity =
            crate::providers::operation_metadata::connection_context_identity(
                &bootstrap.bindings,
                &fixed,
            )
            .unwrap();
        Context::new(bootstrap, context.session.clone()).unwrap()
    }

    fn media_context(context: &Context, kind: RequestKind, settings: Value) -> Context {
        let mut bootstrap = (*context.bootstrap).clone();
        let call = &mut bootstrap.resolution.calls[0];
        call.selection.as_mut().unwrap().request_kind = kind;
        call.settings = serde_json::from_value(settings).unwrap();
        bootstrap.policy.allowed = vec![call.selection.clone().unwrap()];
        bootstrap.resolution.required_selections = bootstrap.policy.allowed.clone();
        Context::new(bootstrap, context.session.clone()).unwrap()
    }

    fn exact_thinking_context(context: &Context, value: &str) -> Context {
        selected_thinking_context(
            context,
            ThinkingSetting::Choice {
                value: value.into(),
            },
        )
    }

    fn selected_thinking_context(context: &Context, setting: ThinkingSetting) -> Context {
        let mut bootstrap = (*context.bootstrap).clone();
        let wire = serde_json::to_value(&setting).unwrap();
        bootstrap.bindings.bindings[0]
            .settings
            .insert("thinking".into(), wire.clone());
        let call = &mut bootstrap.resolution.calls[0];
        call.selection.as_mut().unwrap().thinking = setting;
        call.settings.insert("thinking".into(), wire);
        call.evidence.clear();
        call.compatibility = Compatibility::Unknown;
        bootstrap.policy.allowed = vec![call.selection.clone().unwrap()];
        bootstrap.resolution.required_selections = bootstrap.policy.allowed.clone();
        bootstrap.resolution.binding_identity = canonical_identity(&bootstrap.bindings).unwrap();
        Context::new(bootstrap, context.session.clone()).unwrap()
    }

    #[tokio::test]
    async fn native_ollama_boolean_controls_match_only_their_exact_wire_mapping() {
        for (setting, wire) in [
            (ThinkingSetting::On, "medium"),
            (ThinkingSetting::Off, "none"),
        ] {
            let mut server = mockito::Server::new_async().await;
            let mock = server
                .mock("POST", mockito::Matcher::Any)
                .match_request(move |request| {
                    let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                    body["reasoning_effort"] == wire && body.get("think").is_none()
                })
                .with_status(400)
                .with_body(r#"{"error":{"message":"synthetic provider rejection"}}"#)
                .expect(1)
                .create_async()
                .await;
            let (context, root, _home_guard) = fixture_at_provider(&server.url(), "ollama");
            let context = selected_thinking_context(&bound_role_context(&context), setting);
            for actual in ["unexpected", wire] {
                let result = context
                    .scope(crate::providers::send_text_request_with_account_context(
                        crate::providers::ProviderKind::Ollama,
                        &server.url(),
                        crate::providers::ProviderTextRequest {
                            rubric_enabled: false,
                            model: "fixture",
                            content_parts: &[crate::providers::runtime::ContentPart::Text(
                                "input".into(),
                            )],
                            timeout_in_sec: 5,
                            token: "isolated-fixture",
                            response_schema: &json!({"type":"object","properties":{}}),
                            max_output_tokens: None,
                            temperature: None,
                            thinking: Some(actual),
                        },
                        None,
                    ))
                    .await;
                assert!(result.is_err());
                if actual != wire {
                    assert!(result
                        .unwrap_err()
                        .to_string()
                        .contains("authorization rejected"));
                }
            }
            mock.assert_async().await;
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test]
    async fn native_unimplemented_reasoning_fails_without_dispatch_or_defaulting() {
        use crate::providers::{ProviderErrorKind, ProviderKind};
        for (provider, server_name) in [
            (ProviderKind::OpenAi, "openai"),
            (ProviderKind::Ollama, "ollama"),
            (ProviderKind::Xai, "xai"),
            (ProviderKind::Mistral, "mistral"),
            (ProviderKind::TypeSafe, "typesafe"),
        ] {
            let mut server = mockito::Server::new_async().await;
            let untouched = server
                .mock("POST", mockito::Matcher::Any)
                .expect(0)
                .create_async()
                .await;
            let (context, root, _home_guard) = fixture_at_provider(&server.url(), server_name);
            let context = exact_thinking_context(&bound_role_context(&context), "Custom-Attempt");
            let result = if provider == ProviderKind::TypeSafe {
                context
                    .scope(crate::providers::send_text_request_with_account_context(
                        provider,
                        &server.url(),
                        crate::providers::ProviderTextRequest {
                            rubric_enabled: false,
                            model: "fixture",
                            content_parts: &[crate::providers::runtime::ContentPart::Text(
                                "input".into(),
                            )],
                            timeout_in_sec: 5,
                            token: "isolated-fixture",
                            response_schema: &json!({"type":"object","properties":{}}),
                            max_output_tokens: None,
                            temperature: None,
                            thinking: Some("Custom-Attempt"),
                        },
                        None,
                    ))
                    .await
                    .map(|_| ())
            } else {
                let context = media_context(
                    &context,
                    RequestKind::Image,
                    json!({"format":"png","thinking":{"mode":"choice","value":"Custom-Attempt"}}),
                );
                context
                    .scope(crate::providers::send_image_request_with_account_context(
                        provider,
                        &server.url(),
                        "fixture",
                        "image",
                        5,
                        "isolated-fixture",
                        "png",
                        &[],
                        None,
                        Some("Custom-Attempt"),
                        None,
                    ))
                    .await
                    .map(|_| ())
            };
            let error = result.unwrap_err();
            assert_eq!(error.kind(), ProviderErrorKind::InvalidRequest);
            assert!(
                error
                    .to_string()
                    .contains("cannot serialize an exact native reasoning choice"),
                "{error}"
            );
            untouched.assert_async().await;
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test]
    async fn native_gemini_image_preserves_custom_reasoning_despite_unknown_evidence() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1beta/interactions")
            .match_request(|request| {
                let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                body["generation_config"]["thinking_level"] == "Custom-Attempt"
                    && body["model"] == "fixture"
            })
            .with_status(400)
            .with_body(r#"{"error":{"message":"synthetic provider rejection"}}"#)
            .create_async()
            .await;
        let (context, root, _home_guard) = fixture_at_provider(&server.url(), "gemini");
        let context = exact_thinking_context(&bound_role_context(&context), "Custom-Attempt");
        let context = media_context(
            &context,
            RequestKind::Image,
            json!({"format":"jpeg","thinking":{"mode":"choice","value":"Custom-Attempt"}}),
        );
        let error = context
            .scope(crate::providers::send_image_request_with_account_context(
                crate::providers::ProviderKind::Gemini,
                &server.url(),
                "fixture",
                "image",
                5,
                "isolated-fixture",
                "jpeg",
                &[],
                None,
                Some("Custom-Attempt"),
                None,
            ))
            .await
            .unwrap_err();
        assert_eq!(
            error.kind(),
            crate::providers::ProviderErrorKind::InvalidRequest
        );
        mock.assert_async().await;
        let snapshot = context.session.snapshot();
        assert_eq!(snapshot["outcomes"].as_array().unwrap().len(), 1);
        assert_eq!(snapshot["outcomes"][0]["boundary"]["call_site"], "root");
        assert!(snapshot["outcomes"][0]["cause"]
            .as_str()
            .unwrap()
            .starts_with("provider."));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn native_media_rejects_mismatched_credential_before_admission() {
        use crate::providers::{ProviderKind, ProviderSpeechRequest, ProviderTranscriptionRequest};
        let (context, root, _home_guard) = fixture();
        let image_context = media_context(&context, RequestKind::Image, json!({"format":"png"}));
        assert!(image_context
            .scope(crate::providers::send_image_request_with_account_context(
                ProviderKind::OpenAi,
                "http://127.0.0.1:1",
                "fixture",
                "image",
                5,
                "other-token",
                "png",
                &[],
                None,
                None,
                None,
            ))
            .await
            .is_err());
        let audio_context = media_context(
            &context,
            RequestKind::Audio,
            json!({"voice":"alloy","format":"wav"}),
        );
        assert!(audio_context
            .scope(crate::providers::send_speech_request(
                ProviderKind::OpenAi,
                "http://127.0.0.1:1",
                ProviderSpeechRequest {
                    model: Some("fixture"),
                    text: "speech",
                    voice: "alloy",
                    format: "wav",
                    timeout_in_sec: 5,
                    token: "other-token",
                },
            ))
            .await
            .is_err());
        let transcription_context = media_context(&context, RequestKind::Transcription, json!({}));
        assert!(transcription_context
            .scope(crate::providers::send_transcription_request(
                ProviderKind::OpenAi,
                "http://127.0.0.1:1",
                ProviderTranscriptionRequest {
                    model: "fixture",
                    filename: "recording.wav",
                    audio_bytes: b"audio",
                    media_type: "audio/wav",
                    timeout_in_sec: 5,
                    token: "other-token",
                },
            ))
            .await
            .is_err());
        let snapshot = context.session.snapshot();
        let outcomes = snapshot["outcomes"].as_array().unwrap();
        assert_eq!(outcomes.len(), 3);
        assert!(outcomes
            .iter()
            .all(|outcome| outcome["cause"] == "role.stale_connection"
                && outcome["state"] == "not_dispatched"));
        assert!(snapshot["outstanding"].as_object().unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn native_conflict_and_source_drift_fail_before_network() {
        let (context, root, _home_guard) = fixture();
        assert!(context
            .scope(request("http://127.0.0.1:1", "different"))
            .await
            .is_err());
        assert_eq!(
            context.session.snapshot()["outcomes"][0]["cause"],
            "role.request_conflict"
        );
        std::fs::write(root.join("agent.json"), "changed").unwrap();
        assert!(context
            .scope(request("http://127.0.0.1:1", "fixture"))
            .await
            .is_err());
        assert!(context.session.snapshot()["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["cause"] == "role.package_changed"));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn imported_policy_grants_or_broader_limits_are_rejected() {
        let (context, root, _home_guard) = fixture();
        let mut bootstrap = (*context.bootstrap).clone();
        bootstrap.policy.allowed.push(AllowedSelection {
            profile: Some("other-action".into()),
            request_kind: RequestKind::Text,
            model: ModelSelection::Named {
                value: "fixture".into(),
            },
            thinking: ThinkingSetting::ProviderDefault,
        });
        assert!(Context::new(bootstrap, context.session.clone()).is_err());
        let mut bootstrap = (*context.bootstrap).clone();
        bootstrap.policy.limits.max_output_tokens += 1;
        assert!(Context::new(bootstrap, context.session.clone()).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
