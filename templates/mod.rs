const BUILD_RS_NAME: &str = "build.rs";
const BUILD_RS_TEMPLATE: &str = include_str!("build.rs");

const AGENTCFG_NAME: &str = ".agentcfg";
const AGENTCFG_TEMPLATE: &str = include_str!(".agentcfg");

const MANIFEST_NAME: &str = "Cargo.toml";
const MANIFEST_TEMPLATE: &str = include_str!("Cargo.toml");

const MAIN_RS_NAME: &str = "src/main.rs";
const MAIN_RS_TEMPLATE: &str = include_str!("src/main.rs");

const ARGS_RS_NAME: &str = "src/args.rs";
const ARGS_RS_TEMPLATE: &str = include_str!("src/args.rs");

const CARGO_RS_NAME: &str = "src/cargo.rs";
const CARGO_RS_TEMPLATE: &str = include_str!("src/cargo.rs");

const LIB_RS_NAME: &str = "src/lib.rs";
const LIB_RS_TEMPLATE: &str = include_str!("src/lib.rs");

const OLLAMA_API_CLIENT_RS_NAME: &str = "src/ollama_api_client.rs";
const OLLAMA_API_CLIENT_RS_TEMPLATE: &str = include_str!("src/ollama_api_client.rs");

const OPENAI_API_CLIENT_RS_NAME: &str = "src/openai_api_client.rs";
const OPENAI_API_CLIENT_RS_TEMPLATE: &str = include_str!("src/openai_api_client.rs");

pub const TEMPLATES: [(&str, &str); 9] = [
    (BUILD_RS_NAME, BUILD_RS_TEMPLATE),
    (AGENTCFG_NAME, AGENTCFG_TEMPLATE),
    (MANIFEST_NAME, MANIFEST_TEMPLATE),
    (MAIN_RS_NAME, MAIN_RS_TEMPLATE),
    (ARGS_RS_NAME, ARGS_RS_TEMPLATE),
    (CARGO_RS_NAME, CARGO_RS_TEMPLATE),
    (LIB_RS_NAME, LIB_RS_TEMPLATE),
    (OLLAMA_API_CLIENT_RS_NAME, OLLAMA_API_CLIENT_RS_TEMPLATE),
    (OPENAI_API_CLIENT_RS_NAME, OPENAI_API_CLIENT_RS_TEMPLATE),
];