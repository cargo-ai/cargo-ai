//! Build-time codegen entrypoint for scaffolded agents.
//!
//! Logic is shared with the root crate build via `build_support.rs`.
mod build_support;

fn main() -> Result<(), build_support::BuildError> {
    build_support::run_agent_codegen_with_build_provenance()
}
