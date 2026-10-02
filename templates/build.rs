//! Build-time codegen entrypoint for scaffolded agents.
//!
//! Logic is shared with the root crate build via `build_support.rs`.
mod build_support;
#[path = "src/generated_capabilities/record.rs"]
mod generated_capability_record;

fn main() -> Result<(), build_support::BuildError> {
    println!("cargo:rustc-check-cfg=cfg(cargo_ai_cli)");
    build_support::run_agent_codegen_with_build_provenance()
}
