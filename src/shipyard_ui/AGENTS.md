# Shipyard UI Agent Rules

These rules apply to work under `cargo-ai/src/shipyard_ui/`.

## Primary Goal
- Build and iterate on Shipyard as a high-quality, cross-platform desktop UI for Cargo AI.
- Prefer UI/UX best practices by default when the user has not explicitly overridden behavior.

## Scope Boundaries
- Keep implementation changes inside `cargo-ai/src/shipyard_ui/` whenever possible.
- Allow minimal integration-only changes in:
  - `cargo-ai/src/args.rs`
  - `cargo-ai/src/main.rs`
- Do not spread Shipyard-specific behavior into unrelated modules unless explicitly requested.

## Architecture Expectations
- Keep module responsibilities clear (`app`, `ui`, `config`, `state`, `runtime`).
- Keep tunable UI values centralized in `config.rs`.
- Keep runtime command intents explicit and allowlisted (no free-form command execution model).

## Readability Standards
- Optimize for developer readability and fast human traversal first.
- Do not collapse Shipyard logic into one large file; keep files focused and scoped by responsibility.
- Keep modules and functions small enough to review without excessive scrolling.
- Prefer explicit names over clever abbreviations.
- When behavior becomes complex, add short, behavior-focused comments that explain intent.

## Security Defaults
- Treat Shipyard as a UI front-end over CLI behavior, not a place for privileged backend logic.
- Use direct process execution only; do not execute through shell interpolation.
- Keep execution feed read-only by default (no interactive stdin passthrough) unless explicitly requested.
- Avoid adding logic that could silently run destructive commands.
- Surface command, output, and status clearly for auditability.

## UX Defaults
- Prioritize clarity, legibility, and predictable layout behavior across macOS, Windows, and Linux.
- Use responsive sizing with safe min/max constraints.
- Persist user-adjusted layout preferences where appropriate.

## Documentation Sync
- When architecture or behavior changes, keep these in sync:
  - `cargo-ai/src/shipyard_ui/README.md`
  - `docs/project/stories/CAI-245-moonshot-rust-ui-exploratory.md`
