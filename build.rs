//! Build-time entrypoint for the root `cargo-ai` crate.
//!
//! Shared parsing/mapping/codegen lives in `templates/build_support.rs` so the
//! root build and scaffolded-agent build stay behavior-identical.
#[path = "templates/build_support.rs"]
mod build_support;
#[path = "src/generated_capabilities/record.rs"]
mod generated_capability_record;

use build_support::TemplateSource;

const _: &str = include_str!("templates/build_support.rs");

const BUILD_RERUN_PATHS: &[&str] = &[
    "src/usage_store.rs",
    "src/usage_log.rs",
    "src/usage_attribution.rs",
    ".agentcfg",
    "build.rs",
    "templates",
    "templates/build_support.rs",
];

const TEMPLATE_SOURCES: &[TemplateSource] = &[
    TemplateSource {
        destination: "src/credentials/access_continuity.rs",
        source: "../src/credentials/access_continuity.rs",
    },
    TemplateSource {
        destination: "src/role_transport.rs",
        source: "src/role_transport.rs",
    },
    TemplateSource {
        destination: "src/role_child.rs",
        source: "src/role_child.rs",
    },
    TemplateSource {
        destination: "src/owned_process_windows.rs",
        source: "src/owned_process_windows.rs",
    },
    TemplateSource {
        destination: "src/role_runtime.rs",
        source: "src/role_runtime.rs",
    },
    TemplateSource {
        destination: "src/role_contract.rs",
        source: "src/role_contract.rs",
    },
    TemplateSource {
        destination: "src/role_session.rs",
        source: "src/role_session.rs",
    },
    TemplateSource {
        destination: "src/credentials/role_context.rs",
        source: "src/credentials/role_context.rs",
    },
    TemplateSource {
        destination: "src/providers/operation_metadata.rs",
        source: "../src/providers/operation_metadata.rs",
    },
    TemplateSource {
        destination: "src/providers/thinking.rs",
        source: "../src/providers/thinking.rs",
    },
    TemplateSource {
        destination: "src/providers/thinking_metadata.rs",
        source: "../src/providers/thinking_metadata.rs",
    },
    TemplateSource {
        destination: "src/providers/thinking_image.rs",
        source: "../src/providers/thinking_image.rs",
    },
    TemplateSource {
        destination: "src/execution_policy.rs",
        source: "src/execution_policy.rs",
    },
    TemplateSource {
        destination: "src/runtime_thinking.rs",
        source: "src/runtime_thinking.rs",
    },
    TemplateSource {
        destination: "src/generated_capabilities.rs",
        source: "../src/generated_capabilities.rs",
    },
    TemplateSource {
        destination: "src/generated_capabilities/record.rs",
        source: "../src/generated_capabilities/record.rs",
    },
    TemplateSource {
        destination: "src/usage_backup.rs",
        source: "src/usage_backup.rs",
    },
    TemplateSource {
        destination: "src/usage_backup_host.rs",
        source: "src/usage_backup_host.rs",
    },
    TemplateSource {
        destination: "src/usage_backup/attribution.rs",
        source: "src/usage_backup/attribution.rs",
    },
    TemplateSource {
        destination: "src/usage_backup/queue.rs",
        source: "src/usage_backup/queue.rs",
    },
    TemplateSource {
        destination: "src/usage_backup/projection.rs",
        source: "src/usage_backup/projection.rs",
    },
    TemplateSource {
        destination: "src/usage_backup/transport.rs",
        source: "src/usage_backup/transport.rs",
    },
    TemplateSource {
        destination: "src/usage_store.rs",
        source: "../src/usage_store.rs",
    },
    TemplateSource {
        destination: "build.rs",
        source: "build.rs",
    },
    TemplateSource {
        destination: "build_support.rs",
        source: "build_support.rs",
    },
    TemplateSource {
        destination: "src/result_capture.rs",
        source: "src/result_capture.rs",
    },
    TemplateSource {
        destination: "src/business_schema.rs",
        source: "src/business_schema.rs",
    },
    TemplateSource {
        destination: "definition_validation.rs",
        source: "definition_validation.rs",
    },
    TemplateSource {
        destination: ".agentcfg",
        source: ".agentcfg",
    },
    TemplateSource {
        destination: "Cargo.toml",
        source: "Targo.toml",
    },
    TemplateSource {
        destination: "src/runtime_media.rs",
        source: "src/runtime_media.rs",
    },
    TemplateSource {
        destination: "src/providers/media.rs",
        source: "../src/providers/media.rs",
    },
    TemplateSource {
        destination: "src/providers/image.rs",
        source: "../src/providers/image.rs",
    },
    TemplateSource {
        destination: "src/runtime_data.rs",
        source: "src/runtime_data.rs",
    },
    TemplateSource {
        destination: "src/main.rs",
        source: "src/main.rs",
    },
    TemplateSource {
        destination: "src/args.rs",
        source: "src/args.rs",
    },
    TemplateSource {
        destination: "src/usage_log.rs",
        source: "../src/usage_log.rs",
    },
    TemplateSource {
        destination: "src/usage_attribution.rs",
        source: "../src/usage_attribution.rs",
    },
    TemplateSource {
        destination: "src/web_resources.rs",
        source: "src/web_resources.rs",
    },
    TemplateSource {
        destination: "src/providers/mod.rs",
        source: "src/providers/mod.rs",
    },
    TemplateSource {
        destination: "src/providers/runtime.rs",
        source: "src/providers/runtime.rs",
    },
    TemplateSource {
        destination: "src/providers/anthropic.rs",
        source: "src/providers/anthropic.rs",
    },
    TemplateSource {
        destination: "src/providers/gemini.rs",
        source: "src/providers/gemini.rs",
    },
    TemplateSource {
        destination: "src/providers/openai.rs",
        source: "src/providers/openai.rs",
    },
    TemplateSource {
        destination: "src/providers/openai_compatible.rs",
        source: "src/providers/openai_compatible.rs",
    },
    TemplateSource {
        destination: "src/providers/ollama.rs",
        source: "src/providers/ollama.rs",
    },
    TemplateSource {
        destination: "src/providers/error.rs",
        source: "src/providers/error.rs",
    },
    TemplateSource {
        destination: "src/providers/typesafe.rs",
        source: "src/providers/typesafe.rs",
    },
    TemplateSource {
        destination: "src/providers/compatibility.rs",
        source: "src/providers/compatibility.rs",
    },
    TemplateSource {
        destination: "src/providers/xai.rs",
        source: "src/providers/xai.rs",
    },
    TemplateSource {
        destination: "src/config/loader.rs",
        source: "src/config/loader.rs",
    },
    TemplateSource {
        destination: "src/config/mod.rs",
        source: "src/config/mod.rs",
    },
    TemplateSource {
        destination: "src/config/schema.rs",
        source: "src/config/schema.rs",
    },
    TemplateSource {
        destination: "src/credentials/mod.rs",
        source: "src/credentials/mod.rs",
    },
    TemplateSource {
        destination: "src/credentials/store.rs",
        source: "src/credentials/store.rs",
    },
];

fn main() -> Result<(), build_support::BuildError> {
    println!("cargo:rustc-check-cfg=cfg(cargo_ai_cli)");
    println!("cargo:rustc-cfg=cargo_ai_cli");
    build_support::run_agent_codegen(BUILD_RERUN_PATHS)?;
    let record = generated_capability_record::encoded_record(true);
    let bytes = record
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!(
        "static CLI_RUN_RUNTIME_CAPABILITY_RECORD: [u8; {}] = [{bytes}];\n",
        record.len()
    );
    std::fs::write(
        std::path::PathBuf::from(std::env::var("OUT_DIR").map_err(|source| {
            build_support::BuildError::EnvVar {
                name: "OUT_DIR",
                source,
            }
        })?)
        .join("cli_run_runtime_capabilities.rs"),
        source,
    )
    .map_err(|source| build_support::BuildError::Io {
        context: "Failed to write runtime capability source".into(),
        source,
    })?;

    build_support::write_generated_templates(TEMPLATE_SOURCES)?;
    Ok(())
}
