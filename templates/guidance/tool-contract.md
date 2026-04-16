# Cargo AI Tool Contract

Use this file for the detailed local-tool contract after the basic workflow in `tool-authoring.md` is clear.

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

Treat `.cargo-ai/tools/...` and `.cargo-ai/agents/...` as Cargo AI-owned generated state.
Do not manually `cp`, `mv`, `ln`, or `rm` files inside those directories during normal debugging or validation.

For the current MVP, assume:
- one logical tool
- one Cargo crate
- one primary binary target

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
- the `describe.result` schema itself must still be a nullable string

## Lint Versus Check

`cargo ai tools lint <tool_name>` is the static source/scaffold lint for project-local source-backed tools. It checks managed metadata linkage and scaffold/layout expectations without executing the tool's business logic.

`cargo ai tools check <tool_name>` validates the built binary contract.

Keep them separate:
- use `lint` for source-backed scaffold/layout/metadata conformance
- use `check` for built `describe` contract validation

## Preferred Validation Ladder

Use Cargo AI surfaces as the primary integration validation path for local tools.

Recommended order:
- `cargo test --manifest-path tools/<tool_name>/Cargo.toml`
  - only for crate-local Rust logic and helper behavior
- `cargo ai tools lint <tool_name>`
- `cargo ai tools build <tool_name> --target <triple>`
- `cargo ai tools check <tool_name>`
- `cargo ai tools check --config <agent.json>`
- `cargo ai hatch <agent-name> --config <agent.json> --check`
- live leaf runtime check first when applicable
- live parent runtime check next
- real side effects last

Use leaf-first live testing when runtime behavior depends on:
- URL fetch behavior
- provider or model behavior
- tool-to-child orchestration
- child-process cleanup

That usually means:
- hatch or run the child path first in print or smoke mode
- validate representative inputs there
- then run the parent orchestration path
- only then enable side effects such as email delivery

If the final workflow mixes deterministic fan-out logic with live web inputs:
- prove the deterministic path first with hardcoded or otherwise controlled inputs
- add live URLs only after the tool-to-child path is already green
- leave real side effects for the end

`cargo test`, `cargo ai tools lint`, `cargo ai tools check`, and `cargo ai hatch --check` are necessary static gates, but they do not prove live URL fetches, provider responses, or child-process lifecycle behavior.

## Process Hygiene

Do not use `ps`, `kill`, or similar process-management commands as part of normal tool validation.

Those commands are only justified when all of the following are true:
- a prior live run likely launched a long-lived process
- that process came from your own test command or the tool/agent path you are currently validating
- the leftover process is now blocking, stalling, or contaminating later validation

When cleanup is needed:
- inspect first
- target specific PIDs tied to your own test commands
- explain why cleanup is necessary before doing it
- do not use broad kill patterns
- do not inspect or terminate unrelated system processes

## Managed State Ownership

`.cargo-ai/tools/...` and `.cargo-ai/agents/...` are Cargo AI-owned generated state, not author-owned source folders.

Do not manually:
- replace managed binaries with `cp`
- rename or move them with `mv`
- create symlinks into those directories for normal validation
- delete managed artifacts with `rm` as part of ad hoc debugging

If you manually mutate managed state, treat the workspace as contaminated for further diagnosis.
Do not keep attributing later failures in that workspace to Cargo AI until you reproduce them from:
- a fresh workspace, or
- freshly regenerated managed state with no manual edits afterward

When diagnosing managed-artifact issues:
- prefer `cargo ai tools describe <tool_name>` and `cargo ai tools check <tool_name>` first
- if you run the managed binary path directly, do it only as a read-only diagnostic immediately after a fresh `cargo ai tools build`
- do not mutate the managed artifact path while debugging the problem you are trying to diagnose

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
