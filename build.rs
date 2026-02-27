// Build-time codegen entrypoint for cargo-ai.
// Shared parsing/mapping/codegen logic lives in `templates/build_support.rs`
// so the top-level build and scaffolded agent build stay behavior-identical.
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
        destination: "src/cargo.rs",
        source: "src/cargo.rs",
    },
    TemplateSource {
        destination: "src/lib.rs",
        source: "src/lib.rs",
    },
    TemplateSource {
        destination: "src/ollama_api_client.rs",
        source: "src/ollama_api_client.rs",
    },
    TemplateSource {
        destination: "src/openai_api_client.rs",
        source: "src/openai_api_client.rs",
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
