# Author Agent Definitions

[Documentation hub](./README.md) · [Project README](../README.md)

An agent definition is a JSON document that tells Cargo AI what input to send to a model, what structured result to require, and what actions may follow. Start with the smallest definition that proves the workflow, run it directly while editing, and hatch it only when validation succeeds.

This guide is the human-oriented overview. The generated [agent definition contract](../templates/guidance/agent-definition-contract.md) is the version-matched offline assistant reference for definition validation.

## Choose A Definition Source

Run a local definition while you iterate:

```bash
cargo ai run ./my_agent.json --profile openai-account
cargo ai run --config ./my_agent.json --profile openai-account
```

Cargo AI also accepts inline JSON and standard input when a file is inconvenient:

```bash
cargo ai run --json '<agent-definition-json>' --profile openai-account
cat ./my_agent.json | cargo ai run --stdin --profile openai-account
```

The same source forms work with `hatch`:

```bash
cargo ai hatch my_agent --config ./my_agent.json
cargo ai hatch my_agent --json '<agent-definition-json>'
cat ./my_agent.json | cargo ai hatch my_agent --stdin
```

Prefer a checked-in JSON file for a definition that other people will review, hatch, package, or maintain.

## Start With The Top-Level Shape

A definition uses this ordered top-level shape:

1. `agent_definition_schema_version` selects the definition contract.
2. optional `inputs` provide ordered model-facing context or named reusable slots.
3. optional `action_execution` chooses top-level action scheduling.
4. optional `runtime_vars` declare typed invocation controls.
5. `agent_schema` defines the structured model result.
6. `actions` define validated follow-up work.

```json
{
  "agent_definition_schema_version": "2026-03-03.r1",
  "inputs": [
    {
      "type": "text",
      "text": "What is 2 + 2? Return the answer as an integer."
    }
  ],
  "agent_schema": {
    "type": "object",
    "properties": {
      "answer": { "type": "integer" }
    }
  },
  "actions": [
    {
      "name": "print_answer",
      "logic": { "==": [{ "var": "answer" }, 4] },
      "run": [
        {
          "kind": "exec",
          "platform": ["macos", "linux"],
          "program": "printf",
          "args": ["The answer is 4.\\n"]
        },
        {
          "kind": "exec",
          "platform": "windows",
          "program": "cmd",
          "args": ["/C", "echo", "The answer is 4."]
        }
      ]
    }
  ]
}
```

`agent_definition_schema_version` identifies the Cargo AI contract used to interpret the JSON. It is not the Cargo AI version, agent version, project version, or package version. Copy it from the current template or generated guidance; do not derive it from the date or invent it. The legacy top-level `version` key is rejected.

## Add Model Inputs

`inputs` is optional. When present, its order is preserved and each item uses one of these types:

- `text`, with optional `text`
- `url`, with optional `url`
- `image`, with optional `path`
- `file`, with optional `path`

An unnamed input must include its value. A named input may include a baked value or act as a required slot to fill at runtime.

```json
{
  "inputs": [
    {
      "type": "text",
      "text": "Use the attached report as the source of truth."
    },
    {
      "name": "quarterly_report",
      "type": "file"
    }
  ]
}
```

Use relative `image` and `file` paths. Cargo AI resolves them from the process's current working directory, and they must remain at that level or below. Absolute paths and parent traversal such as `../` are rejected. A URL input is fetched as ordinary HTTP content; do not assume browser-only JavaScript rendering.

Direct file support depends on the selected provider. Check the [provider guide](./providers/README.md) before choosing a profile for a definition that uses `file` inputs.

## Choose Runtime Input Behavior

Generated agents accept repeatable model-input flags such as `--input-text`, `--input-url`, `--input-image`, and `--input-file`.

By default, anonymous runtime input flags replace the baked `inputs` array for that invocation. Set `--input-mode append` to keep baked inputs first, or `--input-mode prepend` to place runtime inputs first.

```bash
./my_agent \
  --input-mode append \
  --input-file ./reports/q1.pdf
```

If the baked JSON contains the only instruction, do not accidentally replace it with a file alone. Supply a runtime `--input-text` too, or use append/prepend mode.

Use names when an input is part of the workflow contract, will be forwarded to a child agent, or should be independently replaceable. Repeatable `--input-override NAME=VALUE` targets declared names without changing anonymous root-model input selection:

```bash
./my_agent \
  --input-override quarterly_report=./reports/q2.pdf \
  --input-text "Focus on changes since Q1."
```

Named overrides are type-checked. URL overrides require an absolute HTTP(S) URL, and file overrides require a supported file extension.

## Declare Invocation Controls

Use `runtime_vars` for typed operational settings that should change without editing the definition. Supported types are `string`, `number`, `integer`, and `boolean`.

```json
{
  "runtime_vars": {
    "score_threshold": { "type": "number", "default": 0.8 },
    "generate_images": { "type": "boolean", "default": false }
  }
}
```

Pass values with repeatable `--run-var name=value` flags and reference them as `runtime.<name>`:

```bash
./my_agent \
  --run-var score_threshold=0.9 \
  --run-var generate_images=true
```

Runtime variable names are flat and must be declared. Undeclared or duplicate flags fail, and a variable without a default must be supplied if an executed path resolves it. Quote values when the shell would otherwise split or interpret them.

## Design The Output Contract

`agent_schema` must be an object with `properties`. Start with top-level `string`, `integer`, `number`, or `boolean` fields. Add descriptions, string enums, and numeric bounds only when they improve the contract.

```json
{
  "agent_schema": {
    "type": "object",
    "properties": {
      "summary": {
        "type": "string",
        "description": "A short operator-facing summary."
      },
      "needs_review": {
        "type": "boolean",
        "description": "Whether a human should review the result."
      }
    }
  }
}
```

Cargo AI also supports a bounded top-level `array`/`object` lane for structured tool parameters:

- arrays must be homogeneous
- objects must declare their shape
- arrays may contain supported scalars or declared-shape objects
- object properties may be scalar or `scalar | null` within these structured payloads
- nested arrays, deeper nested objects, and broader unions are unsupported

Structured top-level fields may flow only into tool parameters as raw JSON. Scalar-first surfaces—including `logic`, `when`, `exec.args`, interpolated strings, `email_me`, and child `run_vars`—reject structured field references. See the [agent definition contract](../templates/guidance/agent-definition-contract.md) and [tool workflow](./projects-and-tools.md) before using this lane.

## Build A Structural Action-Only Agent

Set `agent_schema.properties` to an empty object when the workflow should skip the initial model call and start at actions.

In this shape:

- top-level inputs are optional, but every declared input must be named
- named inputs are reusable parent-owned slots rather than root-model input
- anonymous runtime `--input-*` flags are invalid
- `--input-override NAME=VALUE` can satisfy a named slot
- action `logic` begins with declared `runtime.*` values only
- model-output field references are invalid because no model output exists

This is useful for orchestration workers whose actions call child agents or tools. Read [Actions And Child Agents](./actions-and-child-agents.md) before adding the run steps.

## Validate Before Building

Run the check loop after each meaningful edit:

```bash
cargo ai hatch my_agent --config ./my_agent.json --check
```

`--check` validates the scaffold and compile path with `cargo check` without exporting a binary. Fix errors one at a time, then run the definition directly before producing the final executable.

For exhaustive offline details, use the version-matched guidance shipped with Cargo AI:

- [Agent definition contract](../templates/guidance/agent-definition-contract.md)
- [Action rules](../templates/guidance/action-rules.md)
- [Authoring patterns](../templates/guidance/authoring-patterns.md)
- [Runnable examples](../templates/guidance/examples/README.md)
- [Generated troubleshooting](../templates/guidance/troubleshooting.md)

You can add that guidance to a Cargo AI project with `cargo ai add guidance`; see [Getting Started](./getting-started.md) for the project workflow.

## Related Documentation

- [Documentation hub](./README.md)
- [Project README](../README.md)
- [Actions And Child Agents](./actions-and-child-agents.md)
- [Projects And Local Tools](./projects-and-tools.md)
- [Troubleshooting](./troubleshooting.md)
