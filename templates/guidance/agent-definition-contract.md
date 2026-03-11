# Cargo AI Agent Definition Contract

Use this file as the complete offline contract when you need to author or review a Cargo AI agent definition without looking at repository code.

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
- `status_variable`
- `error_variable`

`exec` also supports:
- `output_variable`

## Step Outcome Rules

- Steps stop the action by default when they fail.
- Use `failure_mode: "continue"` only when a later step should react to a failure.
- `status_variable` stores:
  - `succeeded`
  - `failed`
- `error_variable` stores a human-readable failure string when the step fails.
- If `when` evaluates false, the step is skipped and `status_variable` / `error_variable` stay unset in the MVP.

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
{ "kind": "exec", "program": "echo", "args": ["hello"] }
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
