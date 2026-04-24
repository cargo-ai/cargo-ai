# Cargo AI Versioning

Cargo AI uses semantic versions as a product and CLI contract, not as a Rust library API contract.

## Pre-1.0 Policy

While Cargo AI is in the `0.y.z` range, the leftmost non-zero digit is the main compatibility signal.

- `0.y.0` means meaningful product and contract evolution across one or more public surfaces.
- `0.y.z` means smaller bug fixes, polish, documentation updates, and low-risk compatibility improvements that should not materially change how users author, hatch, run, or share agents.

## How To Read Releases

- A release such as `0.1.1` should feel like a smaller follow-up release, not a major expansion of what authored agents or the CLI can do.
- A release such as `0.2.0` signals that Cargo AI gained meaningful new capability or compatibility surface, even though the product is still pre-`1.0.0`.
- While Cargo AI remains pre-`1.0.0`, generated-agent provenance/version checks stay intentionally exact. After a meaningful upgrade, existing hatched agents may report `out_of_sync` until they are re-hatched with the newer Cargo AI.

## Public Compatibility Surfaces

For Cargo AI, the compatibility surfaces that matter most are:

- agent JSON definition shape and semantics
- `cargo ai` CLI behavior and flags
- hatch and `--check` behavior
- generated-agent behavior and embedded provenance/version expectations
- documented authoring, account, and release-facing workflows that users are expected to follow

Implementation details may change underneath those surfaces, but changes that materially affect them should be treated as contract changes rather than patch-level noise.

## What 1.0.0 Means

`1.0.0` should mean Cargo AI is ready to stand behind a stable core public contract across the surfaces above. That does not mean the product stops evolving. It means compatibility changes become rarer, more deliberate, and more tightly managed.

## Why This Release Is 0.3.0

The `0.3.0` release is a pre-`1.0.0` contract release because Cargo AI has evolved meaningfully since `0.2.0` in user-visible ways, including:

- direct interpreted execution with `cargo ai run`
- inline and stdin definition sources for fast scripted authoring flows
- explicit runtime output rendering controls
- project-local tool authoring, linting, checking, and build materialization
- project build and source-portable package assembly
- account-backed project list, publish, pull, visibility, and archive workflows
- structured tool parameter support for validated array and object values
- install, Cargo AI Home, package, and release-facing documentation updates

That is larger than a patch-level `0.2.1` release, but it is not yet a `1.0.0` stability promise.

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
