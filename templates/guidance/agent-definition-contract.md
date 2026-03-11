# Cargo AI Agent Definition Contract

Use this file as the complete offline contract when you need to author or review a Cargo AI agent definition without looking at repository code.

Unless a section says otherwise, the supported field lists below are exhaustive for the current MVP contract.

## Required Top-Level Shape

Every agent definition must be a JSON object with these keys in this order:

1. `version`
2. `inputs`
3. `agent_schema`
4. `actions`

## `version`

- Required.
- Non-empty string.
- Format: `YYYY-MM-DD.rN`
- Example: `2026-03-03.r1`

## `inputs`

- Required.
- Array with at least one item.
- Supported input shapes:
  - `text`
    - required: `type`, `text`
  - `url`
    - required: `type`, `url`
  - `image`
    - required: `type`, `path`
  - `file`
    - required: `type`, `path`

Path rules for `image` and `file`:
- Use relative paths only.
- Do not use absolute paths.
- Do not use parent traversal such as `../`.

## Runtime Inputs

Generated agent binaries may also accept runtime input flags such as:
- `--input-text`
- `--input-url`
- `--input-image`
- `--input-file`

These runtime inputs are separate from the JSON `inputs` array:
- JSON `inputs` are baked into the definition as default model-facing inputs.
- Runtime input flags are supplied by the caller when the binary runs.
- If any runtime input flags are used, they replace the full JSON-defined `inputs` array for that run.

Authoring guidance:
- Use JSON `inputs` when the definition should own a fixed instruction or a fixed local file/image path.
- Use runtime flags when the caller should choose the content at invocation time.
- If you use `--input-file` and still need a text instruction, also pass `--input-text` because runtime flags replace the full baked `inputs` array.
- `{"type":"file","path":"..."}` is for a definition-owned fixed file path, not for a caller-selected runtime file.

## `agent_schema`

- Required.
- Must be an object with:
  - `type: "object"`
  - `properties: { ... }`
- Each property must define a `type`.
- Top-level property names are reserved for action-variable lookup. Step-captured variable names cannot reuse them.

Supported top-level property `type` values:
- `string`
- `number`
- `integer`
- `boolean`

Supported optional top-level property metadata and constraints:
- `description` on any supported field
- `enum` on `string` fields only
- `minimum`, `maximum`, `exclusiveMinimum`, and `exclusiveMaximum` on `number` and `integer` fields

Constraint rules:
- `enum` values must be non-empty strings
- `enum` values are exact and case-sensitive
- lower bounds may use `minimum` or `exclusiveMinimum`, but not both
- upper bounds may use `maximum` or `exclusiveMaximum`, but not both

Unsupported schema shapes for the current MVP:
- top-level arrays
- nested objects
- union types

## `actions`

- Required.
- Array with at least one action.
- Each action must contain:
  - `name`
  - `logic`
  - `run`

`logic` uses JSON Logic against the top-level model output object. If it evaluates true, the action's `run` steps execute in order.

## Supported Run-Step Kinds

These documented step kinds are exhaustive for the current MVP.

### `exec`

Required fields:
- `kind`
- `program`
- `args`

### `agent`

Required fields:
- `kind`
- `agent`

### `email_me`

Required fields:
- `kind`
- `subject`
- `text`

## Common Optional Step Fields

These fields are available on every step kind:
- `when`
- `failure_mode`
- `platform`
- `status_variable`
- `error_variable`

`exec` also supports:
- `output_variable`

### `platform`

- Optional.
- Allowed values:
  - `macos`
  - `linux`
  - `windows`
- May be a single string or an array of strings in the executable JSON contract.
- Limits the step to the named runtime platform(s).
- If omitted, the step may run on any supported runtime platform.
- If the current runtime platform does not match, the step is skipped and later matching steps continue in order.

Example:

```json
{
  "kind": "exec",
  "program": "/bin/echo",
  "platform": "macos",
  "args": ["hello"]
}
```

## Returned Output vs Actions

- The top-level `agent_schema` fields are the agent's returned structured output.
- Actions run after the model has produced that top-level output.
- `exec`, `agent`, and `email_me` steps are follow-up side effects or orchestration.
- Action steps do not mutate the returned top-level output object.
- `output_variable`, `status_variable`, and `error_variable` are action-local only.

## Step Outcome Rules

- Steps stop the action by default when they fail.
- Use `failure_mode: "continue"` only when a later step should react to a failure.
- `status_variable` stores:
  - `succeeded`
  - `failed`
- `error_variable` stores a human-readable failure string when the step fails.
- If `when` evaluates false, the step is skipped and `status_variable` / `error_variable` stay unset in the MVP.
- If `platform` does not match the current runtime OS, the step is skipped and `status_variable` / `error_variable` stay unset in the MVP.

## Variable Namespace Rules

Within one action:
- `output_variable`, `status_variable`, and `error_variable` share one flat namespace.
- Dotted names are invalid.
- Captured names cannot collide with top-level `agent_schema` property names.
- Captured names cannot be reused within the same action.

Across different top-level actions:
- The same captured names may be reused.

Variable lookup can read:
- top-level model output fields
- prior `output_variable` values from earlier steps in the same action
- prior `status_variable` values from earlier steps in the same action
- prior `error_variable` values from earlier steps in the same action

## Child-Agent Path Rules

For `kind: "agent"`:
- Use explicit same-level paths such as `./child_reporter`
- Do not use bare names
- Do not use absolute paths
- Do not use `../`

## Child-Agent Data Flow

- A parent action may pass child-agent `inputs`.
- Those child inputs may use dynamic string parts resolved from the parent action-local variable bag.
- A parent may observe child-agent success or failure through `status_variable` and `error_variable`.
- A parent cannot directly capture the child agent's top-level returned output fields into its own action-local variables.
- Treat child agents as orchestration/control-flow steps with input forwarding, not as automatic structured-output merging into the parent.

## Validation Expectations

Expect `cargo ai hatch <agent-name> --config <config.json> --check` to reject at least these cases:
- missing required top-level keys
- malformed `version`
- unsupported top-level `agent_schema` property types
- top-level arrays, nested objects, or union types in `agent_schema`
- invalid `description`, `enum`, or numeric-bound metadata on a property
- unsupported run-step kind
- missing required fields for a step kind
- invalid child-agent path shape
- invalid relative file or image paths
- invalid `platform` value
- `output_variable` on non-`exec` steps
- captured-variable collisions
- malformed `when`
- malformed `failure_mode`

## Minimal Valid Examples

```json
{
  "type": "object",
  "properties": {
    "unit": {
      "type": "string",
      "description": "Temperature unit.",
      "enum": ["F", "C"]
    },
    "confidence": {
      "type": "number",
      "description": "Confidence score greater than 0 and less than or equal to 1.",
      "exclusiveMinimum": 0,
      "maximum": 1
    }
  }
}
```

```json
{ "kind": "agent", "agent": "./child_reporter" }
```

```json
{ "kind": "email_me", "subject": "Done", "text": "Finished." }
```

```json
{ "kind": "exec", "program": "/bin/echo", "args": ["hello"], "platform": "macos" }
```

## Default Authoring Loop

1. Draft or edit one JSON file.
2. Run `cargo ai hatch <agent-name> --config <config.json> --check`.
3. Fix reported errors.
4. Build only after `--check` passes.

For onboarding and pattern selection, read:
- `start-here.md`
- `pattern-selection.md`
- `examples/README.md`
