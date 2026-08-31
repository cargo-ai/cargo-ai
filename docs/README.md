# Cargo AI Documentation

Cargo AI turns readable JSON agent definitions into directly runnable workflows and native CLI executables. Use this documentation by goal; the public [README](../README.md) is the shorter product overview and first-success path.

## Start

- [Install Cargo AI](./install/README.md)
- [Build and run your first agent](./getting-started.md)
- [Choose and configure a model provider](./providers/README.md)

## Build

- [Define inputs and structured output](./agent-definitions.md)
- [Add actions and child agents](./actions-and-child-agents.md)
- [Work with projects and local tools](./projects-and-tools.md)

## Share and Manage

- [Build, install, and publish packages](./packages.md)
- [Use accounts and share agent definitions](./accounts-and-sharing.md)
- [Understand Cargo AI Home](./cargo-ai-home.md)

## Help

- [Troubleshoot common problems](./troubleshooting.md)

## Maintain and Release

- [Review testing and Product Qualification](./testing-and-release-qualification.md)
- [Understand versioning and releases](../VERSIONING.md)

## How These Docs Stay Aligned

The repository has three documentation surfaces with different jobs:

- `README.md` is the product landing page and one complete first-success path.
- `docs/` is the canonical human navigation, setup, and operational guidance.
- `templates/guidance/` is the version-matched offline authoring bundle installed by `cargo ai add guidance`. It is self-contained so an AI coding assistant can author and validate agents without access to this repository.

Human guides summarize concepts and link to the active generated-guidance sources when a field-by-field or step-by-step contract is needed. The generated bundle remains authoritative for offline assistant authoring behavior; human setup and workflow guidance remains authoritative here.

The older `templates/shared/docs/` paths are compatibility pointers, not another source of truth.

## Project Links

- [Public README](../README.md)
- [Example agent definitions](../adder_test.json)
- [Release notes](../releases/)
- [MIT license](../LICENSE)
