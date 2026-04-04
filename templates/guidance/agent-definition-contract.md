# Cargo AI Agent Definition Contract

Use this file as the complete offline contract when you need to author or review a Cargo AI agent definition without looking at repository code.
That JSON definition is the source for a generated CLI executable: it defines the inputs, structured output, and follow-up actions the hatched tool will use.

Unless a section says otherwise, the supported field lists below are exhaustive for the current MVP contract.

## Required Top-Level Shape

Every agent definition must be a JSON object with these keys in this order:

1. `version`
2. optional `inputs`
3. optional `action_execution`
4. optional `runtime_vars`
5. `agent_schema`
6. `actions`

## `version`

- Required.
- Non-empty string.
- Format: `YYYY-MM-DD.rN`
- Example: `2026-03-03.r1`

## `inputs`

- Optional.
- If present, must be an array with at least one item.
- Each top-level input may also declare optional `name`.
- For readability, prefer named input object field order as `name`, then `type`, then the value-bearing field (`text`, `url`, or `path`).
- For unnamed literal inputs, keep `type` first and the value-bearing field second.
- Supported input shapes:
  - `text`
    - required: `type`
    - optional: `name`, `text`
  - `url`
    - required: `type`
    - optional: `name`, `url`
  - `image`
    - required: `type`
    - optional: `name`, `path`
  - `file`
    - required: `type`
    - optional: `name`, `path`

Top-level input rules:
- Unnamed top-level inputs must include a baked value (`text`, `url`, or `path`).
- Named top-level inputs may either include a baked value or act as a required slot with no baked value.
- Named top-level inputs are still normal model-facing inputs for schema-backed agents.
- Named top-level inputs may also be reused explicitly by child-agent steps and targeted by `--input-override`.
- Top-level named inputs are literal-or-empty-slot only in the current slice; do not use `runtime.*` or other dynamic expressions there.

Path rules for `image` and `file`:
- Use relative paths only.
- Do not use absolute paths.
- Do not use parent traversal such as `../`.

URL guidance:
- `{"type":"url","url":"..."}` means Cargo AI fetches the URL with its own HTTP client and passes the returned body as text input.
- Treat the practical compatibility target as comparable to `curl` for ordinary static or server-rendered content.
- Do not assume browser-only or JavaScript-rendered pages will work through `type: "url"`.

## Runtime Inputs

Generated agent binaries may also accept runtime input flags such as:
- `--input-mode`
- `--input-text`
- `--input-url`
- `--input-image`
- `--input-file`
- `--input-override NAME=VALUE`

These runtime inputs are separate from the JSON `inputs` array:
- JSON `inputs` are baked into the definition as default model-facing inputs.
- Runtime input flags are supplied by the caller when the binary runs.
- If runtime input flags are used without `--input-mode`, they replace the full JSON-defined `inputs` array for that run.
- `--input-mode replace` explicitly selects runtime-only replacement.
- `--input-mode append` keeps baked inputs first and appends runtime inputs in CLI order.
- `--input-mode prepend` keeps runtime inputs in CLI order first and then places baked inputs after them.
- `--input-override NAME=VALUE` targets one declared named top-level input and replaces that named binding for the current run.
- `--input-override` is repeatable; one binding per flag, split on the first `=`, and later duplicates win.
- `--input-override` type-checks `VALUE` against the declared named input kind:
  - `text`: raw string, including empty string
  - `url`: must be an absolute `http://` or `https://` URL
  - `image`: treated like runtime `--input-image`
  - `file`: treated like runtime `--input-file` and must use a supported file extension
- Anonymous runtime `--input-*` flags do not bind named input identities.
- For schema-backed agents, anonymous runtime `--input-*` flags still control the effective root model input list exactly as before.
- For structural action-only agents, anonymous runtime `--input-*` flags remain invalid; use named top-level inputs plus `--input-override` instead.

Authoring guidance:
- Use JSON `inputs` when the definition should own a fixed instruction or a fixed local file/image path.
- Prefer named top-level inputs when a value is part of the workflow contract, reusable by child-agent steps, or overrideable by name.
- Leave one-off root-model context unnamed when it does not need child reuse or targeted override behavior.
- Use runtime flags when the caller should choose the content at invocation time.
- If you use `--input-file` and still need a text instruction, either also pass `--input-text` in replace mode or choose `--input-mode append` / `--input-mode prepend` so the baked text instruction is still included.
- `{"type":"file","path":"..."}` is for a definition-owned fixed file path, not for a caller-selected runtime file.
- If `agent_schema.properties` is empty, runtime `--input-*` flags are invalid because Cargo AI skips the model call in that structural action-only shape.

## `runtime_vars`

- Optional.
- Object keyed by runtime variable name.
- Use when the caller should control action behavior or a step-local setting at invocation time without editing the JSON.

Each `runtime_vars.<name>` entry supports:
- required `type`
- optional `default`

Supported `type` values:
- `string`
- `number`
- `integer`
- `boolean`

Rules:
- runtime variable names are flat
- runtime variable names cannot contain `.`
- `runtime` and `runtime.*` are reserved
- `default`, when present, must match the declared type
- values are supplied at invocation time through repeatable `--run-var name=value` flags
- undeclared `--run-var` names fail
- duplicate `--run-var` names fail
- if a declared runtime var has no `default`, the caller must supply it when an executed path resolves it
- quote `--run-var` values normally in the caller's shell when they contain spaces or shell-sensitive characters

Example:

```json
{
  "runtime_vars": {
    "generate_images": { "type": "boolean", "default": false },
    "hero_image_model": { "type": "string", "default": "gpt-image-1.5" },
    "score_threshold": { "type": "number", "default": 0.8 }
  }
}
```

## `agent_schema`

- Required.
- Must be an object with:
  - `type: "object"`
  - `properties: { ... }`
- Each property must define a `type`.
- Top-level property names are reserved for action-variable lookup. Step-captured variable names cannot reuse them.
- `properties` may be empty. That declares the structural action-only shape.

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

Structural action-only rule:
- If `agent_schema.properties` is empty, Cargo AI skips the initial model call and begins at the action layer.
- In that shape, top-level `inputs` may still exist, but they must declare `name` because they are reusable parent-owned inputs only.
- In that shape, runtime model-facing `--input-*` flags are invalid.
- In that shape, named top-level inputs may be baked literals or required slots satisfied later by parent pass-through or `--input-override`.
- Top-level action `logic` starts with declared `runtime.*` values only.
- Step `when` and substitution surfaces may use declared `runtime.*` values plus prior captured step variables as the action runs.
- References to top-level model-output fields are invalid in that shape because no initial model output exists.

## `actions`

- Required.
- Array with at least one action.
- Optional top-level `action_execution` may appear alongside `actions`.
  - Allowed values: `sequential`, `parallel`
  - Omitted means `sequential`
  - `parallel` only changes scheduling across matching top-level actions
  - each action's own `run` list remains sequential in both modes
  - a runtime safety/testing override may force the invocation tree down to sequential with `--action-execution sequential`
  - that runtime override is CLI-scoped and inherited by child-agent invocations; parent JSON does not override child JSON
- Each action must contain:
  - `name`
  - `logic`
  - `run`

`logic` uses JSON Logic against the top-level action data object. At action start, that means:
- top-level model output fields plus declared `runtime.*` values for schema-backed agents
- declared `runtime.*` values only for the structural action-only shape

If `logic` evaluates true, the action's `run` steps execute in order.
- In `sequential`, matching top-level actions run one after another.
- In `parallel`, matching top-level actions may overlap, but each action still keeps its own `run` steps in order.
- A hard failure in one top-level action does not prevent later eligible top-level actions from running.
- Cargo AI aggregates top-level hard failures after all eligible actions finish.
- Cargo AI prints one run-level execution header before actions start: `Action execution: sequential` or `Action execution: parallel`.
- Cargo AI also prints one root `using:` line near run start that shows the effective `profile`, `auth`, `server`, and `model` for that invocation.
- It adds `url=...` only when the effective URL is custom or materially different from the standard transport.
- In redirected, piped, CI, or simpler terminal output, Cargo AI prefixes parent-visible action output with deterministic labels such as `[Action 1: generate_images]`.
- In append-only output, long-running steps also emit a step-start liveness line such as `step 2/2 generate_image started; waiting for provider response...`.
- Terminal lane summaries and the final run footer also include wall-clock durations, for example `completed in 31s.` and `✅ Run complete in 32s.`.
- When attached directly to an interactive terminal, Cargo AI switches to a compact live lane dashboard.
- The first live dashboard slice shows lane label, lane status with elapsed time while running and after terminal completion/failure, terminal step marker or current step when known, and the last lifecycle message.
- Parent-run `exec` and `email_me` output is bucketed into the originating lane.
- Child-agent steps stay minimal in the parent lane with start/completion or exit-summary lines instead of recursively inlining the child transcript.
- In append-only output, provider-backed or child-agent steps emit another action-prefixed `using:` line only when the effective `profile`, `auth`, `server`, or `model` changes from the most recently printed context.
- Interactive live mode keeps the parent dashboard at the orchestration level and does not surface child or step-level `using:` lines there.

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

Optional fields:
- `profile`

### `email_me`

Required fields:
- `kind`
- `subject`
- `text`

### `generate_image`

Required fields:
- `kind`
- `prompt`
- `path`
- Optional:
  - `model`
  - `profile`
- First slice writes one local image file.
- If `model` is omitted, Cargo AI falls back to the effective invocation model resolved from the current profile and any `--model` CLI override.
- If `profile` is present, Cargo AI resolves that profile at step runtime and uses it for the image step's provider/url/token context.
- With `generate_image.profile`, explicit `model` still wins, then the step-profile model, then the parent invocation model.
- If neither the step nor the invocation provides a model, the step fails clearly at runtime.
- `model` may be:
  - a literal non-empty string
  - a single variable reference such as `{ "var": "runtime.hero_image_model" }`
  - a single top-level string output field such as `{ "var": "image_model" }`
- `generate_image.model` may not read captured `output_variable`, `status_variable`, or `error_variable` values in the current contract.
- For Cargo AI's default OpenAI account transport, use a tool-capable mainline model such as `gpt-5.2`.
- For a direct OpenAI API token and URL, prefer GPT Image models such as `gpt-image-1.5` or `gpt-image-1-mini`.
- Current-at-ship-date note: official OpenAI docs list `gpt-image-1.5` as the latest GPT Image model, and the image-generation guide lists `gpt-image-1.5`, `gpt-image-1`, and `gpt-image-1-mini` for direct image generation. Verified: 2026-03-28.

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
- `exec`, `agent`, `email_me`, and `generate_image` steps are follow-up side effects or orchestration.
- Action steps do not mutate the returned top-level output object.
- `output_variable`, `status_variable`, and `error_variable` are action-local only.

## Step Outcome Rules

- Steps stop the action by default when they fail.
- Use `failure_mode: "continue"` only when a later step should react to a failure.
- Use `failure_mode: "abort"` when the whole invocation should stop scheduling new work and fail with an explicit abort summary.
- A hard failure stops the rest of that action's `run` list, but it does not prevent later eligible top-level actions from running; Cargo AI aggregates top-level action failures after the full scan.
- In the first `abort` slice, already-running work settles cooperatively unless a safe cancellation path already exists.
- Child-agent abort stays local to the child invocation first; the parent lane then handles that failed child exit according to the parent step's own `failure_mode`.
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
- `runtime` and `runtime.*` are reserved for declared invocation-scoped runtime variables.
- Captured names cannot collide with top-level `agent_schema` property names.
- Captured names cannot be reused within the same action.

Across different top-level actions:
- The same captured names may be reused.

Top-level action `logic` can read:
- top-level model output fields
- declared `runtime.*` values

Step `when` and string/arg substitutions can read:
- top-level model output fields
- declared `runtime.*` values
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
- A parent action may also pass child-agent `run_vars`.
- A parent action may also pass child-agent `input_overrides`.
- If the target is another Cargo AI agent, prefer a native `kind: "agent"` step instead of wrapping the child invocation in Python or shell just to launch it.
- Wrapper scripts are exceptions for genuinely non-Cargo-AI behavior around the call, not the default way to invoke a child agent.
- A `kind: "agent"` run step may also set `profile` as a literal string or single variable reference; Cargo AI resolves it at step runtime and forwards `--profile <name>` to the child.
- `run_vars` is the child-step equivalent of repeatable `--run-var NAME=VALUE`.
- `input_overrides` is the child-step equivalent of repeatable `--input-override NAME=VALUE`.
- A `kind: "agent"` run step may also set `input_mode` to `replace`, `append`, or `prepend` when child `inputs` are present.
- Child `run_vars`, child `input_overrides`, child `inputs`, and child `input_mode` are all optional.
- Child `run_vars` must be an object keyed by the intended child runtime-var name.
- Child `run_vars` values must be scalar literals (`string`, `number`, or `boolean`) or a single variable reference in the exact shape `{ "var": "<name>" }`.
- Child `input_overrides` must be an object keyed by the intended child named-input slot.
- Child `input_overrides` values must be either a string literal, a single variable reference in the exact shape `{ "var": "<name>" }`, or a named parent-input reference in the exact shape `{ "input": "<name>" }`.
- Child `inputs` may contain either literal input objects or a named parent-input reference in the exact shape `{ "input": "<name>" }`.
- `{ "input": "<name>" }` may reference only declared named top-level inputs from the parent definition.
- Cargo AI keeps parent and child agents atomic at hatch/check time; whether a child actually declares a `run_vars` key or `input_overrides` key and whether the resolved values match are checked when the child runs.
- When a child run step provides both `run_vars` and named/input payload values, Cargo AI forwards `run_vars` first, then named `input_overrides`, and then merges anonymous child `inputs` with `input_mode`.
- Child `input_mode` applies only to child `inputs`; it does not change whether `input_overrides` are sent.
- If child `input_mode` is omitted, the child step keeps the current default behavior: child `inputs` replace the child agent's baked `inputs`.
- Child `append` keeps the child agent's baked inputs first, then appends the action-supplied child inputs in declared order.
- Native child-agent steps preserve Cargo AI semantics such as child `run_vars`, named input forwarding, child `input_overrides`, child `inputs`, `input_mode`, failure handling, depth limits, and step-level `using:` observability.
- Child `prepend` keeps the action-supplied child inputs first in declared order, then places the child agent's baked inputs after them.
- Those child inputs may use dynamic string parts resolved from the parent action-local variable bag. Child `run_vars` and child `input_overrides` stay CLI-shaped and accept only scalar literals or single variable references.
- Named child-input reuse is explicit only; Cargo AI does not automatically inherit all named inputs into children.
- If a child only consumes a forwarded named input, it does not need to declare the same name locally.
- If a child wants to forward that same named input to its own child, it should declare the same named top-level input locally so the incoming value can bind there first.
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
- malformed child-agent `input_overrides`
- malformed child-agent `run_vars`
- invalid child-agent `input_mode`
- duplicate named top-level inputs
- unnamed top-level inputs in the structural action-only shape
- malformed or unknown `{ "input": "<name>" }` child references
- invalid relative file or image paths
- invalid generated-image output path extension
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
