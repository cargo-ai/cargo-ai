# Cargo-AI Submodule Agent Rules

These rules apply to work under `cargo-ai/`.

## Build/Check Command
- On Apple Silicon macOS, run checks with:
  - `cargo check --target aarch64-apple-darwin`
- Plain `cargo check` may resolve to a cross target from Cargo configuration and fail due host/tooling mismatch.

## Validation Expectation
- When reporting verification for `cargo-ai`, include the exact command that was run.
