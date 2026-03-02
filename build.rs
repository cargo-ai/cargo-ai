//! Build-time entrypoint for the root `cargo-ai` crate.
//!
//! Shared parsing/mapping/codegen lives in `templates/build_support.rs` so the
//! root build and scaffolded-agent build stay behavior-identical.
#[path = "templates/build_support.rs"]
mod build_support;

use build_support::TemplateSource;

const TEMPLATE_SOURCES: &[TemplateSource] = &[
    TemplateSource {
        destination: "build.rs",
        source: "build.rs",
    },
    TemplateSource {
        destination: "build_support.rs",
        source: "build_support.rs",
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
        destination: "src/main.rs",
        source: "src/main.rs",
    },
    TemplateSource {
        destination: "src/args.rs",
        source: "src/args.rs",
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
        destination: "src/providers/openai.rs",
        source: "src/providers/openai.rs",
    },
    TemplateSource {
        destination: "src/providers/ollama.rs",
        source: "src/providers/ollama.rs",
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
];

fn main() -> Result<(), build_support::BuildError> {
    build_support::run_agent_codegen(&[".agentcfg", "templates", "build.rs"])?;
    build_support::write_generated_templates(TEMPLATE_SOURCES)?;
    Ok(())
}
