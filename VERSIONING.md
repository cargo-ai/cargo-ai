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

Useful `0.x` releases continue while real use and user feedback inform that decision. There is no fixed `1.0.0` launch date or requirement to align it with another application release. Security and release qualification remain required for the selected release regardless of its version number.

## Current Release Line

The `0.4.x` line begins with [0.4.0](./releases/0.4.0.md). It introduces the package-first command surface, an explicit agent definition schema key, stricter versioned validation, additional provider adapters, and expanded package ownership and sharing behavior.

This is a compatibility boundary from `0.3.x`. Read the migration notes before upgrading existing definitions, scripts or generated agents. A version bump does not automatically migrate user-owned source or rebuild installed packages.

## Upgrade Guidance

Cargo AI continues to recommend manual upgrades:

```bash
cargo install cargo-ai
```

After a meaningful pre-`1.0.0` upgrade, re-hatch generated agents if their embedded version/provenance status reports `out_of_sync`.

To check whether a newer crates.io version exists:

```bash
cargo ai version --check
```
