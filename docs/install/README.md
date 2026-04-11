# Install Cargo AI

The preferred install path today is the Cargo-based workflow:

1. install Rust and Cargo
2. run `cargo install cargo-ai --locked`
3. verify with `cargo ai --help`

This path keeps the full CLI available, including `hatch`, and avoids platform-specific standalone packaging differences while Cargo AI's broader distribution story is still being built out.

## Recommended Today

Use the Cargo install guide here:

- [Install with Cargo](./cargo.md)

## Current Platform Posture

- macOS: Cargo-first is the preferred install path today.
- Linux: Cargo-first is the preferred install path today.
- Windows: Cargo-first works today; standalone Windows distribution is a later install/distribution phase.

## Related Docs

- [Cargo AI Home](../cargo-ai-home.md)
- [Versioning](../../VERSIONING.md)
