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
