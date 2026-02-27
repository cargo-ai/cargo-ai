// Build-time codegen entrypoint for scaffolded agents.
// Logic is shared with cargo-ai's root build script via `build_support.rs`.
mod build_support;

fn main() -> Result<(), build_support::BuildError> {
    build_support::run_agent_codegen(&[".agentcfg", "build.rs", "build_support.rs"])
}
