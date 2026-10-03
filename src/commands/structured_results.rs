//! Invocation-local selected producer validation and publication.
use crate::business_schema::{decode_producer_result, validate_nominations, ResultError};
use serde_json::Value;

#[derive(Clone)]
pub(crate) struct Context {
    declaration: Option<Value>,
    publish: bool,
}
tokio::task_local! { static CONTEXT: Option<Context>; }
pub(crate) async fn scope<F: std::future::Future>(
    context: Option<Context>,
    future: F,
) -> F::Output {
    CONTEXT.scope(context, future).await
}
pub(crate) fn current() -> Option<Context> {
    CONTEXT.try_with(Clone::clone).ok().flatten()
}
impl Context {
    pub(crate) fn new(declaration: Option<Value>, publish: bool) -> Self {
        Self {
            declaration,
            publish,
        }
    }
    fn scopes(&self) -> Vec<String> {
        self.declaration
            .as_ref()
            .and_then(|d| d.get("artifact_scopes"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    }
    pub(crate) fn preflight(&self) -> Result<(), String> {
        let scopes = self.scopes();
        if !scopes.is_empty() {
            if !self.publish {
                return Err("Artifact scopes require an authorized root action invocation.".into());
            }
            if super::client_actions::validate_result_artifact_scopes(&scopes).is_err() {
                let error=ResultError {code:"artifact.access_denied",message:"Declared artifact scopes require an authorized root action and native permission."};
                return Err(self.failure(error));
            }
        }
        Ok(())
    }
    fn failure(&self, error: ResultError) -> String {
        if self.publish {
            let mut failure = super::machine::Failure::new(error.code, error.message);
            if error.code == "artifact.export_failed" {
                failure = failure.with_data(serde_json::json!({"partial":true}));
            }
            super::machine::record_error(failure);
        }
        error.to_string()
    }
}
pub(crate) fn capture(raw: Option<&str>) -> Result<(), String> {
    let context = current().ok_or("Selected producer has no invocation result context.")?;
    let declaration = context
        .declaration
        .as_ref()
        .ok_or("Selected producer has no root result declaration.")?;
    let result = decode_producer_result(raw, &declaration["schema"])
        .map_err(|error| context.failure(error))?;
    if context.publish {
        super::machine::record_result(&result.data);
    }
    let nominations = result.artifacts.map_err(|error| context.failure(error))?;
    if validate_nominations(&nominations).is_err() {
        return Err(context.failure(ResultError { code:"artifact.export_failed",message:"The selected producer artifact nominations are invalid or exceed supported limits." }));
    }
    if !context.publish {
        if !nominations.is_empty() {
            return Err("Descendant results cannot export root action artifacts.".into());
        }
        return Ok(());
    }
    super::client_actions::export_result_artifacts(&nominations, &context.scopes())
}

pub(crate) fn protocol_failure() -> String {
    let error = ResultError {
        code: "runtime.result_invalid",
        message: "The selected producer returned an invalid tool protocol response.",
    };
    current().map_or_else(|| error.to_string(), |context| context.failure(error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn declaration() -> Value {
        json!({"source":"tool","schema":{"type":"integer"}})
    }
    #[tokio::test]
    async fn each_invocation_replaces_the_parent_context_and_parallel_lanes_share_only_explicit_context(
    ) {
        assert!(current().is_none());
        scope(Some(Context::new(Some(declaration()),true)),async {
            assert!(current().unwrap().publish);
            scope(Some(Context::new(Some(declaration()),false)),async {
                assert!(!current().unwrap().publish);
                assert!(capture(Some(r#"{"data":7}"#)).is_ok());
                assert!(capture(Some(r#"{"data":7,"artifacts":[{"id":"a","scope":"exports","path":"a.txt","mime_type":"text/plain"}]}"#)).is_err());
            }).await;
            assert!(current().unwrap().publish);
            let inherited=current();
            tokio::spawn(async move {assert!(current().is_none());scope(inherited,async {assert!(current().unwrap().publish);}).await;}).await.unwrap();
        }).await;
        assert!(current().is_none());
    }
    #[test]
    fn descendants_cannot_declare_root_artifact_scopes() {
        let context = Context::new(
            Some(
                json!({"source":"tool","schema":{"type":"integer"},"artifact_scopes":["exports"]}),
            ),
            false,
        );
        assert!(context.preflight().is_err());
    }
}

pub(crate) fn limit_failure() -> String {
    let error = ResultError {
        code: "runtime.result_limit",
        message: "The selected producer exceeded its bounded subprocess channel.",
    };
    current().map_or_else(|| error.to_string(), |context| context.failure(error))
}
