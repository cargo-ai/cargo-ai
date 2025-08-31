const BUILD_RS_NAME: &str = "build.rs";
const BUILD_RS_TEMPLATE: &str = include_str!("build.rs");

const AGENTCFG_NAME: &str = ".agentcfg";
const AGENTCFG_TEMPLATE: &str = include_str!(".agentcfg");

const MANIFEST_NAME: &str = "Cargo.toml";
const MANIFEST_TEMPLATE: &str = include_str!("Cargo.toml");

const MAIN_NAME: &str = "src/main.rs";
const MAIN_TEMPLATE: &str = include_str!("src/main.rs");

pub const TEMPLATES: [(&str, &str); 4] = [
    (BUILD_RS_NAME, BUILD_RS_TEMPLATE),
    (AGENTCFG_NAME, AGENTCFG_TEMPLATE),
    (MANIFEST_NAME, MANIFEST_TEMPLATE),
    (MAIN_NAME, MAIN_TEMPLATE),
];