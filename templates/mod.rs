//! Embedded source templates used when scaffolding agent projects.

const BUILD_RS_NAME: &str = "build.rs";
const BUILD_RS_TEMPLATE: &str = include_str!("build.rs");

const BUILD_SUPPORT_RS_NAME: &str = "build_support.rs";
const BUILD_SUPPORT_RS_TEMPLATE: &str = include_str!("build_support.rs");

const AGENTCFG_NAME: &str = ".agentcfg";
const AGENTCFG_TEMPLATE: &str = include_str!(".agentcfg");

const MANIFEST_NAME: &str = "Cargo.toml";
const MANIFEST_TEMPLATE: &str = include_str!("Targo.toml");

const MAIN_RS_NAME: &str = "src/main.rs";
const MAIN_RS_TEMPLATE: &str = include_str!("src/main.rs");

const ARGS_RS_NAME: &str = "src/args.rs";
const ARGS_RS_TEMPLATE: &str = include_str!("src/args.rs");

const USAGE_LOG_RS_NAME: &str = "src/usage_log.rs";
const USAGE_LOG_RS_TEMPLATE: &str = include_str!("src/usage_log.rs");

const PROVIDERS_MOD_RS_NAME: &str = "src/providers/mod.rs";
const PROVIDERS_MOD_RS_TEMPLATE: &str = include_str!("src/providers/mod.rs");

const PROVIDERS_RUNTIME_RS_NAME: &str = "src/providers/runtime.rs";
const PROVIDERS_RUNTIME_RS_TEMPLATE: &str = include_str!("src/providers/runtime.rs");

const PROVIDERS_ANTHROPIC_RS_NAME: &str = "src/providers/anthropic.rs";
const PROVIDERS_ANTHROPIC_RS_TEMPLATE: &str = include_str!("src/providers/anthropic.rs");

const PROVIDERS_OPENAI_RS_NAME: &str = "src/providers/openai.rs";
const PROVIDERS_OPENAI_RS_TEMPLATE: &str = include_str!("src/providers/openai.rs");

const PROVIDERS_OLLAMA_RS_NAME: &str = "src/providers/ollama.rs";
const PROVIDERS_OLLAMA_RS_TEMPLATE: &str = include_str!("src/providers/ollama.rs");

const PROVIDERS_ERROR_RS_NAME: &str = "src/providers/error.rs";
const PROVIDERS_ERROR_RS_TEMPLATE: &str = include_str!("src/providers/error.rs");

const CONFIG_LOADER_RS_NAME: &str = "src/config/loader.rs";
const CONFIG_LOADER_RS_TEMPLATE: &str = include_str!("src/config/loader.rs");

const CONFIG_MOD_RS_NAME: &str = "src/config/mod.rs";
const CONFIG_MOD_RS_TEMPLATE: &str = include_str!("src/config/mod.rs");

const CONFIG_SCHEMA_RS_NAME: &str = "src/config/schema.rs";
const CONFIG_SCHEMA_RS_TEMPLATE: &str = include_str!("src/config/schema.rs");

const CREDENTIALS_MOD_RS_NAME: &str = "src/credentials/mod.rs";
const CREDENTIALS_MOD_RS_TEMPLATE: &str = include_str!("src/credentials/mod.rs");

const CREDENTIALS_STORE_RS_NAME: &str = "src/credentials/store.rs";
const CREDENTIALS_STORE_RS_TEMPLATE: &str = include_str!("src/credentials/store.rs");

/// Template files emitted into a newly scaffolded agent workspace.
pub const TEMPLATES: [(&str, &str); 18] = [
    (BUILD_RS_NAME, BUILD_RS_TEMPLATE),
    (BUILD_SUPPORT_RS_NAME, BUILD_SUPPORT_RS_TEMPLATE),
    (AGENTCFG_NAME, AGENTCFG_TEMPLATE),
    (MANIFEST_NAME, MANIFEST_TEMPLATE),
    (MAIN_RS_NAME, MAIN_RS_TEMPLATE),
    (ARGS_RS_NAME, ARGS_RS_TEMPLATE),
    (USAGE_LOG_RS_NAME, USAGE_LOG_RS_TEMPLATE),
    (PROVIDERS_MOD_RS_NAME, PROVIDERS_MOD_RS_TEMPLATE),
    (PROVIDERS_RUNTIME_RS_NAME, PROVIDERS_RUNTIME_RS_TEMPLATE),
    (PROVIDERS_ANTHROPIC_RS_NAME, PROVIDERS_ANTHROPIC_RS_TEMPLATE),
    (PROVIDERS_OPENAI_RS_NAME, PROVIDERS_OPENAI_RS_TEMPLATE),
    (PROVIDERS_OLLAMA_RS_NAME, PROVIDERS_OLLAMA_RS_TEMPLATE),
    (PROVIDERS_ERROR_RS_NAME, PROVIDERS_ERROR_RS_TEMPLATE),
    (CONFIG_LOADER_RS_NAME, CONFIG_LOADER_RS_TEMPLATE),
    (CONFIG_MOD_RS_NAME, CONFIG_MOD_RS_TEMPLATE),
    (CONFIG_SCHEMA_RS_NAME, CONFIG_SCHEMA_RS_TEMPLATE),
    (CREDENTIALS_MOD_RS_NAME, CREDENTIALS_MOD_RS_TEMPLATE),
    (CREDENTIALS_STORE_RS_NAME, CREDENTIALS_STORE_RS_TEMPLATE),
];
