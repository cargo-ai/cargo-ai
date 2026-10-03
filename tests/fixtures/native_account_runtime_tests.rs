#[cfg(test)]
mod native_account_runtime_tests {
    use super::*;
    use std::path::PathBuf;

    struct Fixture {
        root: PathBuf,
        original_home: Option<std::ffi::OsString>,
        original_codex: Option<std::ffi::OsString>,
        original_disable: Option<std::ffi::OsString>,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "native-account-runtime-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(root.join("codex")).unwrap();
            std::fs::create_dir_all(root.join("cargo")).unwrap();
            let original_home = std::env::var_os("CARGO_AI_HOME");
            let original_codex = std::env::var_os("CODEX_HOME");
            let original_disable = std::env::var_os("CARGO_AI_DISABLE_KEYCHAIN");
            unsafe {
                std::env::set_var("CARGO_AI_HOME", root.join("cargo"));
                std::env::set_var("CODEX_HOME", root.join("codex"));
                std::env::set_var("CARGO_AI_DISABLE_KEYCHAIN", "1");
            }
            std::fs::write(root.join("cargo/config.toml"), "[cargo_ai_metadata]\ncargo_ai_install_id = \"11111111-1111-4111-8111-111111111111\"\n\n[[profile]]\nname = \"native-image\"\nserver = \"openai\"\nmodel = \"selected-image\"\nauth_mode = \"openai_account\"\n").unwrap();
            std::fs::create_dir_all(root.join(".cargo-ai")).unwrap();
            std::fs::write(
                root.join(".cargo-ai/project.toml"),
                "[runtime]\ndata_root = \".cargo-ai/data\"\n",
            )
            .unwrap();
            Self {
                root,
                original_home,
                original_codex,
                original_disable,
            }
        }
        #[cfg(unix)]
        fn use_linked_auth(&self) {
            let auth = self.root.join("codex/auth.json");
            std::fs::rename(&auth, self.root.join("codex/auth-target.json")).unwrap();
            std::os::unix::fs::symlink("auth-target.json", &auth).unwrap();
            std::os::unix::fs::symlink(self.root.join("codex"), self.root.join("codex-linked"))
                .unwrap();
            unsafe {
                std::env::set_var("CODEX_HOME", self.root.join("codex-linked"));
            }
        }
        fn auth(&self, account: Option<&str>) -> Vec<u8> {
            let payload = serde_json::json!({"tokens":{"access_token":"synthetic-runtime-token", "account_id":account}}).to_string().into_bytes();
            std::fs::write(self.root.join("codex/auth.json"), &payload).unwrap();
            payload
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe {
                match &self.original_home {
                    Some(value) => std::env::set_var("CARGO_AI_HOME", value),
                    None => std::env::remove_var("CARGO_AI_HOME"),
                }
                match &self.original_codex {
                    Some(value) => std::env::set_var("CODEX_HOME", value),
                    None => std::env::remove_var("CODEX_HOME"),
                }
            }
            unsafe {
                match &self.original_disable {
                    Some(value) => std::env::set_var("CARGO_AI_DISABLE_KEYCHAIN", value),
                    None => std::env::remove_var("CARGO_AI_DISABLE_KEYCHAIN"),
                }
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn native_account_optional_context_is_bounded_and_debug_redacted() {
        for context in [
            serde_json::Value::Null,
            serde_json::json!("synthetic-context"),
        ] {
            let raw = serde_json::json!({"tokens":{"access_token":"synthetic-token","account_id":context}}).to_string();
            let session = parse_codex_session(&raw).unwrap();
            let debug = format!("{session:?}");
            assert!(!debug.contains("synthetic-token"));
            assert!(!debug.contains("synthetic-context"));
        }
        for context in [
            serde_json::json!(""),
            serde_json::json!("bad context"),
            serde_json::json!("x".repeat(257)),
            serde_json::json!("bad\ncontext"),
            serde_json::json!(true),
        ] {
            let raw = serde_json::json!({"tokens":{"access_token":"synthetic-token","account_id":context}}).to_string();
            assert!(parse_codex_session(&raw).is_err());
        }
        assert!(parse_codex_session(&"x".repeat(64 * 1024 + 1)).is_err());
        assert!(!codex_access_token_expired_or_near(None));
        assert!(codex_access_token_expired_or_near(Some(1)));
    }

    #[tokio::test]
    async fn native_account_selected_session_text_image_override_and_legacy() {
        let fixture = Fixture::new();
        for account in [
            Some("synthetic-personal-context"),
            Some("synthetic-workspace-context"),
            None,
        ] {
            #[cfg(unix)]
            if account == Some("synthetic-workspace-context") {
                fixture.use_linked_auth();
            }
            let auth_before = fixture.auth(account);
            let config_before = std::fs::read(fixture.root.join("cargo/config.toml")).unwrap();
            let selected = SelectedProfile {
                name: "native-image".into(),
                auth_mode: ProfileAuthMode::OpenaiAccount,
                legacy_token: None,
            };
            let resolved =
                resolve_openai_token_for_request(Some(&selected), load_config().as_ref())
                    .await
                    .unwrap();
            assert_eq!(resolved.openai_account_id.as_deref(), account);
            assert!(!format!("{resolved:?}").contains("synthetic-runtime-token"));
            if let Some(account) = account {
                assert!(!format!("{resolved:?}").contains(account));
            }
            let override_context = resolve_media_step_profile_context(
                Some(&RunArg::Literal("native-image".into())),
                &serde_json::json!({}),
                "image",
                9,
                "generate_image",
                None,
                None,
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(override_context.openai_account_id.as_deref(), account);
            assert_eq!(override_context.inference_timeout_in_sec, 9);
            assert!(!format!("{override_context:?}").contains("synthetic-runtime-token"));
            for image in [false, true] {
                let mut server = mockito::Server::new_async().await;
                let body = if image {
                    "data: {\"item\":{\"type\":\"image_generation_call\",\"result\":\"c2FmZS1pbWFnZQ==\"}}\n\n"
                } else {
                    "data: {\"type\":\"response.output_text.done\",\"text\":\"{\\\"status\\\":\\\"ok\\\"}\"}\n\n"
                };
                let mock = server
                    .mock("POST", "/native")
                    .match_header("authorization", "Bearer synthetic-runtime-token")
                    .match_header(
                        "chatgpt-account-id",
                        account.map_or(mockito::Matcher::Missing, |value| {
                            mockito::Matcher::Exact(value.into())
                        }),
                    )
                    .with_status(200)
                    .with_body(body)
                    .create_async()
                    .await;
                let _endpoint = crate::providers::openai::native_account_test_endpoint(format!(
                    "{}/native",
                    server.url()
                ));
                if image {
                    let step: RunStep = serde_json::from_value(serde_json::json!({
                        "kind":"generate_image", "profile":{"Literal":"native-image"}, "prompt":[{"Literal":"draw"}],
                        "path":[{"Literal":"native-image.png"}], "args":[], "tool_params":{}, "ignore_tools":false
                    })).unwrap();
                    let mut parent_context = override_context.clone();
                    parent_context.token = "wrong-parent-token".into();
                    parent_context.openai_account_id = Some("wrong-parent-context".into());
                    parent_context.project_data =
                        runtime_data::project_data_root(Some(&fixture.root)).unwrap();
                    assert!(parent_context.project_data.is_some());
                    run_generate_image_step(
                        &step,
                        &serde_json::json!({}),
                        &std::collections::BTreeMap::new(),
                        0,
                        "image",
                        1,
                        &parent_context,
                        configured_agent_action_runtime_budget_with_project_default(Some(9), None),
                    )
                    .await
                    .unwrap();
                    assert_eq!(
                        std::fs::read(fixture.root.join(".cargo-ai/data/native-image.png"))
                            .unwrap(),
                        b"safe-image"
                    );
                } else {
                    let matches = args::build_cli_from(vec![
                        "generated".into(),
                        "--profile".into(),
                        "native-image".into(),
                    ]);
                    run_with_matches(matches).await;
                }
                mock.assert_async().await;
            }
            assert_eq!(
                std::fs::read(fixture.root.join("codex/auth.json")).unwrap(),
                auth_before
            );
            assert_eq!(
                std::fs::read(fixture.root.join("cargo/config.toml")).unwrap(),
                config_before
            );
            #[cfg(unix)]
            if account == Some("synthetic-workspace-context") {
                assert!(
                    std::fs::symlink_metadata(fixture.root.join("codex/auth.json"))
                        .unwrap()
                        .file_type()
                        .is_symlink()
                );
                assert!(std::fs::symlink_metadata(fixture.root.join("codex-linked"))
                    .unwrap()
                    .file_type()
                    .is_symlink());
                assert_eq!(
                    std::fs::read(fixture.root.join("codex/auth-target.json")).unwrap(),
                    auth_before
                );
            }
        }
        std::fs::write(
            fixture.root.join("codex/auth.json"),
            b"invalid private auth payload",
        )
        .unwrap();
        let api = SelectedProfile {
            name: "manual-api".into(),
            auth_mode: ProfileAuthMode::ApiKey,
            legacy_token: Some("synthetic-api-key".into()),
        };
        let api = resolve_openai_token_for_request(Some(&api), load_config().as_ref())
            .await
            .unwrap();
        assert_eq!(api.token, "synthetic-api-key");
        assert!(api.openai_account_id.is_none());
        assert!(!api.uses_account_session);
    }
}
