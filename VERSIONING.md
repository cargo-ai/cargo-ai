# Cargo AI Versioning

Cargo AI uses semantic versions as a product and CLI contract, not as a Rust library API contract.

## Pre-1.0 Policy

While Cargo AI is in the `0.y.z` range, the leftmost non-zero digit is the main compatibility signal.

- `0.y.z` means a compatible release on the same pre-`1.0.0` line. It may include bug fixes, polish, documentation updates, and additive capabilities that should not require users to rewrite existing agent definitions, CLI usage, generated agents, or project workflows.
- `0.(y+1).0` means a new pre-`1.0.0` compatibility line. Cargo AI uses this for larger milestones, compatibility-boundary releases, or changes with meaningful migration risk.

## How To Read Releases

- A release such as `0.3.1` should be compatible with the `0.3.0` line. It may add new optional behavior, but existing supported definitions and commands should keep working.
- A release such as `0.4.0` signals a new pre-`1.0.0` compatibility line or a larger product milestone where users should read the release notes before assuming the same compatibility posture as `0.3.x`.
- While Cargo AI remains pre-`1.0.0`, generated-agent provenance/version checks stay intentionally exact. After installing a newer Cargo AI, existing hatched agents may report `out_of_sync` until they are re-hatched with the newer Cargo AI, even when the release is otherwise compatible.

## Public Compatibility Surfaces

For Cargo AI, the compatibility surfaces that matter most are:

- agent JSON definition shape and semantics
- `cargo ai` CLI behavior and flags
- hatch and `--check` behavior
- generated-agent behavior and embedded provenance/version expectations
- documented authoring, account, and release-facing workflows that users are expected to follow

Implementation details may change underneath those surfaces. Compatible additive changes may ship on the same `0.y.z` line, while breaking changes, migration-risky changes, or larger compatibility-boundary milestones should move to the next `0.(y+1).0` line.

## What 1.0.0 Means

`1.0.0` should mean Cargo AI is ready to stand behind a stable core public contract across the surfaces above. That does not mean the product stops evolving. It means compatibility changes become rarer, more deliberate, and more tightly managed.

## Current Release Line

The `0.3.x` line began with the `0.3.0` pre-`1.0.0` contract release. That release introduced meaningful user-visible evolution since `0.2.0`, including:

- direct interpreted execution with `cargo ai run`
- inline and stdin definition sources for fast scripted authoring flows
- explicit runtime output rendering controls
- project-local tool authoring, linting, checking, and build materialization
- project build and source-portable package assembly
- account-backed project list, publish, pull, visibility, and archive workflows
- structured tool parameter support for validated array and object values
- install, Cargo AI Home, package, and release-facing documentation updates

Compatible additive releases on the `0.3.x` line can build on that baseline without signaling a new compatibility boundary. The next `0.4.0` release is reserved for a larger milestone, a compatibility-boundary release, or a change with meaningful migration risk.

## Upgrade Guidance

Cargo AI continues to recommend manual upgrades:

```bash
cargo install cargo-ai --locked
```

After a meaningful pre-`1.0.0` upgrade, re-hatch generated agents if their embedded version/provenance status reports `out_of_sync`.

To check whether a newer crates.io version exists:

```bash
cargo ai version --check
```
