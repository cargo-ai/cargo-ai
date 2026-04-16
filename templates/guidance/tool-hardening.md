# Cargo AI Tool Hardening

Use this file for dependency selection, security review, and completion criteria for local Rust tools.

## Dependency Discipline

Tools are normal Rust crates, so they may use crates.io dependencies when the task needs them. Treat every added crate as trusted executable code, not as harmless data.

When adding dependencies:
- prefer the Rust standard library when it reasonably fits
- choose a stable, well-established crate that directly fits the tool's job
- prefer active maintenance, clear ownership, clear docs, and broad real-world usage
- keep the dependency tree and enabled features as small as practical
- disable default features when they add unnecessary surface area
- avoid broad frameworks when a focused crate solves the need
- avoid Git dependencies, path dependencies, unpublished forks, beta or RC releases, unnecessary build scripts, and unnecessary native dependencies unless the user explicitly accepts the tradeoff
- do not constrain the tool to crates already used by Cargo AI itself; tool domains can differ materially from Cargo AI's own dependency surface

For each meaningful new dependency:
- explain why the crate is needed
- explain why a smaller or standard-library-only approach is insufficient
- update and keep the tool's `Cargo.lock`
- review features with `cargo tree -e features` when practical
- run `cargo audit` and `cargo deny check` when those tools are available; if unavailable, say that explicitly

## Hardening Review

Treat every tool as production local executable code. A tool may start exploratory, but do not present it as complete until it has been hardened.

Before completion, review:
- parameter validation for every declared param
- error messages for invalid input and failed external operations
- whether `resource_profile` accurately declares filesystem, network, subprocess, environment, credential, UI, or background-process behavior
- filesystem paths, including parent traversal, absolute paths, overwrite behavior, and output locations
- network behavior, including timeouts, URLs, authentication, and unexpected data exposure
- subprocess or UI/process launching behavior, including whether it can outlive the tool invocation
- environment variable and credential reads
- dependency risk and feature surface
- failure modes, cleanup behavior, and whether partial output can be mistaken for success

Use normal Rust best practices:
- parse into typed values before business logic
- keep custom behavior in `src/tool.rs`
- keep helper functions small and testable
- keep public surface area narrow
- prefer clear `Result`-based errors over panics
- add comments only for non-obvious behavior or important safety boundaries

## UI And Background-Process Tools

For UI or background-process tools:
- separate rendering or artifact generation from launching the UI when practical
- expose a smoke-test control such as `open_window=false` so automated validation can prove the tool without leaving a process open
- make process lifetime explicit when launching windows, servers, or other child processes
- mark UI launch, subprocess use, filesystem writes, and background behavior accurately in `resource_profile`
- if a live run does leave a long-lived child process behind, treat cleanup as exceptional process hygiene rather than as a normal testing step; inspect first, target only the specific child process from your own run, and explain the cleanup reason before terminating it
