# Cargo AI Action Rules

Use this file as the local source of truth for action-step behavior when authoring JSON definitions offline.

## Top-Level Shape
- Required top-level keys:
  - `version`
  - `inputs`
  - `agent_schema`
  - `actions`

## Supported Step Kinds
- `exec`
  - Required: `kind`, `program`, `args`
- `agent`
  - Required: `kind`, `agent`
- `email_me`
  - Required: `kind`, `subject`, `text`

## Optional Control Fields
- `when`
  - JSON Logic object evaluated by the parent action runner.
- `failure_mode`
  - Allowed values: `stop`, `continue`
  - Omitted means `stop`.
- `status_variable`
  - Stores `succeeded` or `failed` when the step runs.
- `error_variable`
  - Stores a human-readable error string when the step fails.
- `output_variable`
  - `exec` only
  - Stores captured stdout.

## Step Outcome Rules
- Steps stop the action by default when they fail.
- `failure_mode: "continue"` allows later steps to run after failure.
- If a step is skipped because `when` is false, `status_variable` and `error_variable` stay unset in the MVP.

## Variable Namespace Rules
- Captured names are flat. Dotted names are invalid.
- `output_variable`, `status_variable`, and `error_variable` share one action-local namespace.
- Captured names cannot collide with top-level `agent_schema` output field names.
- Captured names cannot be reused within the same action.
- The same captured names may be reused in different top-level actions.

## Variable Lookup Rules
- `when` and string-part substitutions can read:
  - top-level model output fields
  - prior `output_variable` values from earlier steps in the same action
  - prior `status_variable` values from earlier steps in the same action
  - prior `error_variable` values from earlier steps in the same action
- Later actions cannot read captured variables from earlier top-level actions.

## Path Rules
- Child agents must use explicit same-level paths such as `./child_reporter`.
- Local file and image paths should stay relative.
- Parent-directory traversal such as `../` is invalid.

## Check Loop
1. Edit one JSON definition at a time.
2. Run `cargo ai hatch <agent-name> --config <config.json> --check`.
3. Fix validation errors before building.
4. Build only after `--check` passes.
