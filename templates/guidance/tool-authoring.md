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

## Project Preconditions

Tool authoring assumes the workspace is already a Cargo AI project with:

```text
.cargo-ai/project.toml
```

If that metadata is missing, explain that `cargo ai add tool <name>` needs a Cargo AI project first.

## Default Local Tool Workflow

1. Scaffold the tool:
   - `cargo ai add tool <tool_name>`
2. Review the generated crate:
   - `tools/<tool_name>/Cargo.toml`
   - `tools/<tool_name>/src/main.rs`
   - `tools/<tool_name>/src/lib.rs`
   - `.cargo-ai/tools/<tool_name>/tool.json`
3. Implement the tool contract in `src/lib.rs`:
   - `describe`
   - `invoke`
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

## What The Scaffold Means

The generated scaffold is intentionally minimal:

- `src/main.rs`
  - thin CLI entrypoint
  - dispatches `describe` and `invoke`
- `src/lib.rs`
  - owns the request/response models
  - owns `describe` and `invoke`
  - starts with a stub success response of `result: null`
- `.cargo-ai/tools/<tool_name>/tool.json`
  - Cargo AI-managed metadata that points back to the source crate

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
- implement `describe` and `invoke` in `src/lib.rs`
- keep `src/main.rs` thin
- build with `cargo ai tools build <tool_name> --target <triple>`
- validate with `cargo ai tools describe`, `cargo ai tools check`, and `cargo ai hatch --check`

Do not invent a second tool runtime or manifest format beyond the current scaffold and `tool.json`.
