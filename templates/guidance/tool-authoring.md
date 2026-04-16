# Cargo AI Tool Authoring

Use this file when the user wants a project-local tool crate, not just agent JSON.

## When To Reach For A Tool

Prefer normal agent JSON first.

Use a local Cargo AI tool when the user needs:
- deterministic local behavior behind a reusable `kind: "tool"` step
- typed params checked against a tool-owned `describe` contract
- a reusable local capability that multiple agents can call
- something more structured than an ad hoc `exec` wrapper

If plain `exec`, `email_me`, `agent`, or `generate_image` can solve the request cleanly, keep the workflow in JSON and do not invent a tool.

When executable code is needed and Cargo is available, prefer a Rust Cargo AI tool:
- scaffold it with `cargo ai add tool <tool_name>`
- implement behavior inside the generated Rust crate
- avoid creating ad hoc Python, Node, or shell helper scripts by default

Only use another language or standalone script when the user explicitly asks, Cargo is unavailable, or the task cannot reasonably fit the current `describe` / `invoke` tool contract.

## Project Preconditions

Tool authoring assumes the workspace is already a Cargo AI project.

If the project boundary is missing, bootstrap it first:

1. New folder:
   - `cargo ai new <path>`
2. Existing folder:
   - `cargo ai init`
3. If the user wants Codex guidance too:
   - `cargo ai add guidance --style codex`

Current minimum project metadata:

```text
.cargo-ai/project.toml
```

with:

```toml
format_version = 1

[tools]
allow_global_fallback = true
```

`cargo ai new/init` writes that default policy so new projects can reuse machine-level tools when desired. If you hand-author `project.toml` and omit `allow_global_fallback`, Cargo AI treats that as project-only lookup.

## Default Local Tool Workflow

1. Scaffold the tool:
   - `cargo ai add tool <tool_name>`
2. Review the generated crate:
   - `tools/<tool_name>/Cargo.toml`
   - `tools/<tool_name>/src/main.rs`
   - `tools/<tool_name>/src/lib.rs`
   - `tools/<tool_name>/src/agent_bridge.rs`
   - `tools/<tool_name>/src/tool.rs`
   - `.cargo-ai/tools/<tool_name>/tool.json`
3. Implement the tool behavior in `src/tool.rs`:
   - description
   - params
   - result metadata
   - resource profile
   - examples
   - invoke behavior
4. Build the managed artifact:
   - `cargo ai tools build <tool_name> --target <triple>`
5. Inspect and validate it:
   - `cargo ai tools describe <tool_name>`
   - `cargo ai tools check <tool_name>`
6. Wire the tool into agent JSON:
   - `kind: "tool"`
   - `name`
   - `params`
7. Validate the agent/tool pairing:
   - `cargo ai tools check --config <agent.json>`
   - `cargo ai hatch <agent-name> --config <agent.json> --check`

If `cargo ai init/new` was run with the default VCS mode, it will also initialize Git and create or update `.gitignore` for generated guidance and managed build state. If Git is unavailable, rerun with `--vcs none`.

## What The Scaffold Means

The generated scaffold is intentionally minimal:

- `src/main.rs`
  - thin CLI entrypoint
  - dispatches `describe` and `invoke`
- `src/lib.rs`
  - owns the request/response models
  - owns the Cargo AI protocol adapter for `describe` and `invoke`
  - normally does not need edits for tool behavior
- `src/agent_bridge.rs`
  - Cargo AI-owned helper layer for child-agent calls from the tool
  - keeps child-agent argument shaping, depth rules, and runtime-budget propagation out of author code
  - normally does not need edits for tool behavior
- `src/tool.rs`
  - author-owned implementation area
  - owns the tool metadata and `invoke` behavior
  - starts with a stub success response of `result: null`
- `.cargo-ai/tools/<tool_name>/tool.json`
  - Cargo AI-managed metadata that points back to the source crate

For the current MVP, assume:
- one logical tool
- one Cargo crate
- one primary binary target

## Dependency Discipline

Tools are normal Rust crates, so they may use crates.io dependencies when the task needs them. Treat every added crate as trusted executable code, not as harmless data.

When adding dependencies:
- prefer the Rust standard library when it reasonably fits
- choose a stable, well-established crate that directly fits the tool's job
- prefer active maintenance, clear ownership, clear docs, and broad real-world usage
- keep the dependency tree and enabled features as small as practical
- disable default features when they add unnecessary surface area
- avoid broad frameworks when a focused crate solves the need
- avoid Git dependencies, path dependencies, unpublished forks, beta/RC releases, unnecessary build scripts, and unnecessary native dependencies unless the user explicitly accepts the tradeoff
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

For UI or background-process tools:
- separate rendering or artifact generation from launching the UI when practical
- expose a smoke-test control such as `open_window=false` so automated validation can prove the tool without leaving a process open
- make process lifetime explicit when launching windows, servers, or other child processes
- mark UI launch, subprocess use, filesystem writes, and background behavior accurately in `resource_profile`

Use normal Rust best practices:
- parse into typed values before business logic
- keep custom behavior in `src/tool.rs`
- keep helper functions small and testable
- keep public surface area narrow
- prefer clear `Result`-based errors over panics
- add comments only for non-obvious behavior or important safety boundaries

## The Current Tool Contract

The binary must support:

- `describe`
  - prints machine-readable JSON
- `invoke`
  - reads JSON from stdin
  - writes a JSON success envelope to stdout
  - returns non-zero plus stderr on failure

Current `invoke` request shape:

```json
{
  "protocol_version": 1,
  "params": {
    "name": "Cargo AI"
  }
}
```

When Cargo AI calls a tool from a parent `kind: "tool"` step, it may also include an internal optional `runtime_context` block. New scaffolded tools wrap that block as the `InvocationContext` argument passed to `src/tool.rs`.

Current success response shape:

```json
{
  "protocol_version": 1,
  "result": "Hello, Cargo AI!"
}
```

`result` may be `null` when the tool succeeds without a compact string result.

## Tool-To-Agent Handoff

New scaffolded tools receive an `InvocationContext` argument plus a Cargo AI-owned helper in `src/agent_bridge.rs`.

Use that helper when the tool needs procedural control such as:
- splitting or iterating over input values
- calling one or more same-project child agents in sequence
- keeping iterator/fan-out logic in Rust instead of expanding the agent JSON or relying on the model to do deterministic string processing

Use it from `src/tool.rs`:

```rust
let request = ChildAgentRequest::new("./child_agent.json")
    .add_text_input("hello from the tool");
context.invoke_agent(request)?;
```

Current first-slice rules:
- same-project child-agent targets only; use same-level relative paths such as `./child_agent` or `./child_agent.json`
- tool execution itself does not consume an extra agent-depth hop
- a child-agent call from the tool consumes depth exactly as if the parent had called that child directly
- the helper carries the remaining runtime budget through to the child-agent invocation
- manual direct `describe` / `invoke` calls outside a parent Cargo AI tool step will not include child-agent bridge context
- keep custom orchestration in `src/tool.rs`; do not rewrite `src/agent_bridge.rs`

## Agent JSON Wiring

Use a tool step like this:

```json
{
  "kind": "tool",
  "name": "hello_tool",
  "params": {
    "name": "Cargo AI",
    "shout": true
  },
  "output_variable": "greeting"
}
```

Current rules:
- param names must match the tool `describe` contract
- param values may be scalar literals or `{ "var": "field_name" }`
- `output_variable` is optional
- if `output_variable` is set, the tool must return a non-null string result

## Resolution And Failure Behavior

By default:
- `cargo ai run`
- `cargo ai hatch --check`
- `cargo ai hatch`

all perform an upfront tool audit against `describe`.

That means Cargo AI checks:
- the tool exists
- the tool contract can be loaded
- the JSON step params match the declared tool params

`--ignore-tools` only skips that upfront audit.

It does not make a missing tool succeed. If execution reaches a tool step and the tool is still missing or incompatible, that step fails normally.

## Current Storage Model

- source:
  - `tools/<tool_name>/...`
- managed metadata and built artifact:
  - `.cargo-ai/tools/<tool_name>/...`

Project scope resolves before machine scope.

## What To Tell Codex To Do

If the user wants a local tool, ask Codex to:
- scaffold or inspect the local tool crate
- implement tool metadata and behavior in `src/tool.rs`
- keep `src/main.rs` thin
- avoid rewriting the protocol adapter in `src/lib.rs` unless the tool contract itself must change
- apply conservative dependency selection before editing `Cargo.toml`
- perform the hardening review before presenting the tool as complete
- build with `cargo ai tools build <tool_name> --target <triple>`
- validate with `cargo ai tools describe`, `cargo ai tools check`, and `cargo ai hatch --check`

Do not invent a second tool runtime or manifest format beyond the current scaffold and `tool.json`.
