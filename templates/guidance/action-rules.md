# Cargo AI Action Rules

Use this file as the fast local reference for action-step behavior when authoring JSON definitions offline.

For broader shape and validation rules, also read:
- `agent-definition-contract.md`
- `start-here.md`
- `examples/README.md`

## Top-Level Shape
- Required top-level keys:
  - `version`
  - `inputs`
  - `agent_schema`
  - `actions`

## Supported Step Kinds

These documented step kinds and helper fields are exhaustive for the current MVP.

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
- `platform`
  - Allowed values: `macos`, `linux`, `windows`
  - May be a string or array of strings in the executable JSON contract.
  - If omitted, the step is eligible to run on any supported runtime platform.
  - If the current platform does not match, the step is skipped.
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
- If a step is skipped because `platform` does not match, `status_variable` and `error_variable` stay unset in the MVP.
- Matching steps still run in listed order.

## Returned Output vs Actions

- Top-level `agent_schema` fields are the returned output of the agent.
- Action steps are side effects or follow-up orchestration after that output exists.
- `output_variable` captures step-local stdout only. It does not change the returned top-level output object.

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

## Child-Agent Data Flow

- Parent actions may pass child-agent `inputs`, including dynamic string parts resolved from current action-local data.
- Parent `agent` steps may set child `input_mode` to `replace`, `append`, or `prepend` when they also provide child `inputs`.
- Omitted child `input_mode` keeps the current replace behavior for child inputs.
- Parent actions may capture child-agent success/failure with `status_variable` and `error_variable`.
- Parent actions cannot directly capture the child agent's top-level returned output fields into the parent action-local namespace.

## Check Loop
1. Edit one JSON definition at a time.
2. Run `cargo ai hatch <agent-name> --config <config.json> --check`.
3. Fix validation errors before building.
4. Build only after `--check` passes.
