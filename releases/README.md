# Release notes

[Public README](../README.md) · [Documentation](../docs/README.md) · [Versioning](../VERSIONING.md)

Read what changed and any compatibility or upgrade guidance for each version. A source checkout can include notes for a version still being prepared; check [published releases](https://github.com/cargo-ai/cargo-ai/releases) and [release status](../docs/testing-and-release-qualification.md#release-status) for availability and verification.

| Version | Highlights |
| --- | --- |
| [0.4.2](./0.4.2.md) | TypeSafe Jev Choice/Score support, profile-aware hatch checks and explicit-profile failure correction |
| [0.4.1](./0.4.1.md) | Secure stdin credential input for native clients and clearer profile guidance |
| [0.4.0](./0.4.0.md) | Package-first workflows, local ownership and a new definition/command compatibility line |
| [0.3.1](./0.3.1.md) | Image generation, provider behavior and usage-ledger improvements |
| [0.3.0](./0.3.0.md) | Expanded CLI/runtime, project tools, packages and agent definitions |
| [0.2.0](./0.2.0.md) | Earlier CLI and agent-definition capabilities |

Normal installation and manual upgrades use `cargo install cargo-ai`. Review the selected version's notes and [pre-1.0 compatibility policy](../VERSIONING.md#pre-10-policy), especially when moving between minor release lines. Re-hatch generated agents when their embedded provenance reports `out_of_sync`.
