const BUILD_RS_NAME: &str = "build.rs";
const BUILD_RS_TEMPLATE: &str = include_str!("build.rs");

const AGENTCFG_NAME: &str = ".agentcfg";
const AGENTCFG_TEMPLATE: &str = include_str!(".agentcfg");

const MANIFEST_NAME: &str = "Cargo.toml";
const MANIFEST_TEMPLATE: &str = include_str!("Cargo.toml");

pub const TEMPLATES: [(&str, &str); 3] = [
    (BUILD_RS_NAME, BUILD_RS_TEMPLATE),
    (AGENTCFG_NAME, AGENTCFG_TEMPLATE),
    (MANIFEST_NAME, MANIFEST_TEMPLATE),
];