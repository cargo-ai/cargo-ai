# Cargo AI Package Workflow

Use this file when a user wants to turn agents, tools, and assets into a reusable Cargo AI package, install a package locally, or use hosted package version management.

## Authoring Shape

A Cargo AI package is the versioned artifact. It can contain agent definitions, hatched-agent entrypoints, project-local tool source, and assets. Use `agent package` only as shorthand when the package mainly contains agents.

Project identity and build profiles live in `.cargo-ai/project.toml`:

```toml
format_version = 1

[project]
name = "data_integration"
version = "0.1.0"

[tools]
allow_global_fallback = true

[build.default]
agent_definitions = ["agents/lookup_account.json"]
hatched_agents = ["agents/daily_digest.json"]
tools = ["database_query"]
assets = ["assets/prompts/"]
```

Keep build lists explicit. `cargo ai package` and `cargo ai build` do not infer tools or agents from JSON references.

## Build And Local Install

```bash
# Build a portable package root/archive from [build.default]
cargo ai package

# Install the current project package into Cargo AI Home
cargo ai packages install

# Install the current project package with an explicit alias
cargo ai packages install --as data_integration

# Install from a package root, archive, or cargo-ai-package.toml path
cargo ai packages install ./dist/data_integration --as data_integration

# Inspect, run, hatch, or uninstall the alias
cargo ai packages list
cargo ai packages inspect data_integration
cargo ai run data_integration::lookup_account
cargo ai hatch data_integration::daily_digest
cargo ai packages uninstall data_integration
```

Local alias behavior is version-aware: same version and hash is a no-op, newer semver upgrades, older semver requires `--downgrade`, and same-version content replacement or different package identity requires `--replace`.

Bare package names without `--account` are local-only. They must not trigger network lookup.

## Hosted Packages

```bash
# Hosted listings
cargo ai packages list --account
cargo ai packages list --account alice

# Publish the current project package
cargo ai packages publish

# Pull without machine install
cargo ai packages pull data_integration
cargo ai packages pull data_integration --owner-handle alice
cargo ai packages pull data_integration --owner-handle alice --version 1.2.3

# Install hosted packages into Cargo AI Home
cargo ai packages install data_integration --account --as data_integration
cargo ai packages install data_integration --account alice --as data_integration
cargo ai packages install data_integration --account alice --version 1.2.3 --as data_integration

# Accept a reviewed hosted subprocess permission when the package requests it
cargo ai packages install data_integration --account alice --as data_integration --accept-permissions

# Version management for installed hosted aliases
cargo ai packages update data_integration
cargo ai packages rollback data_integration --to 1.1.0
```

Hosted source identity is server/API-owned. Do not derive it from a public URL. `inspect` and hosted pull receipts expose the opaque hosted source id, hosted version id, optional owner handle, resolved version, and package hash.

Omitting `--version` resolves the latest eligible semver version at that moment and pins the exact resolved version locally. `update` moves forward only when a newer eligible version exists. `rollback` targets the exact `--to <version>`; it never means latest.

Published versions for one hosted package identity must increase by semver. If a user needs to change same-version content, they should publish a higher version.

## Installed Layout And Permissions

Installed aliases use:

```text
$CARGO_AI_HOME/packages/<alias>/
  install.toml
  package/
  data/
```

`install.toml` is Cargo AI-owned provenance. `package/` is the verified payload for the active exact version. `data/` is package-owned persistent local state.

Hosted update and rollback rematerialize `install.toml` and `package/`, then preserve `data/`. Do not add publisher-authored migrations or total-refresh behavior unless that is part of a separate, explicit story.

Installed package entrypoints resolve Cargo AI-controlled child `usage_log` paths under `data/`. Use this when a parent/observer agent needs a child-specific metadata-only JSONL file before a package tool imports it into SQLite or another package-owned store.

Default hosted package runtime boundaries:
- `package/` is readable verified payload.
- `data/` is the default writable package-owned root.
- project/workspace writes require an explicit grant and are not implied by install.
- Cargo AI-controlled file writes validate relative paths and reject traversal out of `data/`.
- unconstrained `exec` and tool subprocess steps are blocked for hosted packages unless the installed package permission profile explicitly allows them.
- first install or a version transition that newly requests subprocess execution requires `--accept-permissions` after review.
- project/workspace `read` and `read_write` requests are unsupported and fail even with `--accept-permissions`.
- hosted JSON child agents must be declared package exports; direct child executables require the accepted subprocess permission and resolve only from verified `package/`.
- definition-owned image/file inputs resolve inside verified `package/`; caller filesystem access requires an explicit runtime named-input override.
- package-generated relative child inputs resolve under `data/`; nested agents cannot reinterpret text or generated values as external filesystem paths.

Hatching a hosted alias exports trusted executable code outside the installed permission boundary. Review it and acknowledge that transition explicitly:

```bash
cargo ai hatch data_integration::daily_digest --allow-hosted-code
```

Hosted archives are checked before extraction. The client accepts at most 10 MiB compressed, 100 MiB expanded, 10,000 entries, and 1,024 bytes per normalized relative path.

## Cross-Package References

Use installed aliases and exported entrypoints:

```json
{ "kind": "agent", "agent": "data_integration::lookup_account" }
```

Do not introduce unqualified global lookup by bare agent or tool name. Package internals remain private unless they are exported as entrypoints.
