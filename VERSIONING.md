# Cargo AI Versioning

Cargo AI uses semantic versions as a product and CLI contract, not as a Rust library API contract.

## Pre-1.0 Policy

While Cargo AI is in the `0.y.z` range, the leftmost non-zero digit is the main compatibility signal.

- `0.y.0` means meaningful product and contract evolution across one or more public surfaces.
- `0.y.z` means smaller bug fixes, polish, documentation updates, and low-risk compatibility improvements that should not materially change how users author, hatch, run, or share agents.

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

## Why The Next Release Is 0.2.0

The next release is `0.2.0` because Cargo AI has evolved meaningfully since `0.1.0` in user-visible ways, including:

- typed `runtime_vars` for invocation-scoped action and run-step control
- Ollama-compatible image generation for the existing `generate_image` run-step contract
- additive parent/child workflow contract improvements such as named reusable inputs and child runtime-var forwarding
- parallel top-level action execution as an explicit orchestration mode
- related runtime and validator alignment that changes what valid authored agent definitions can do

That is larger than a patch-level `0.1.1` release, but it is not yet a `1.0.0` stability promise.

## Upgrade Guidance

Cargo AI continues to recommend manual upgrades:

```bash
cargo install cargo-ai --locked
```

To check whether a newer crates.io version exists:

```bash
cargo ai version --check
```
