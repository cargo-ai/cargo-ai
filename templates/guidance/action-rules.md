# Cargo AI Action Rules

Use this file as the fast local reference for action-step behavior when authoring JSON definitions offline.

For broader shape and validation rules, also read:
- `agent-definition-contract.md`
- `start-here.md`
- `examples/README.md`

## Top-Level Shape
- Required top-level keys:
  - `agent_definition_schema_version`
  - optional `inputs`
  - optional `action_execution`
  - optional `runtime_vars`
  - `agent_schema`
  - `actions`

`agent_definition_schema_version` identifies the Cargo AI contract used to interpret the definition. It is not an agent or package version; copy it from the current Cargo AI template or guidance rather than inventing a value from the current date or another version surface.

The current strict revision is `2026-09-09.r1`. Valid earlier revisions keep legacy parsing behavior; every other revision at or after the cutoff is unsupported. Strict stable objects reject additional keys. `inputs` is optional, and `actions: []` is valid for a model-only definition. Each declared action has exactly `name`, `logic`, and a nonempty `run` array.

Each `logic` or `when` object contains one supported operator. Use the exhaustive operator list in `agent-definition-contract.md`; unknown operators and `literal` are rejected. For an unconditional true gate, use `{ "==": [1, 1] }`. Existing scalar-reference, flat-variable and comparison-type rules still apply.

## Top-Level Action Execution
- `action_execution`
  - Allowed values: `sequential`, `parallel`
  - Omitted means `sequential`.
  - `parallel` only changes scheduling across matching top-level actions.
  - Each individual action's `run` list stays sequential in both modes.
- Runtime may force a parallel-capable invocation tree down to sequential with `--action-execution sequential`.
- That runtime override is invocation-scoped and inherits to child-agent steps; parent JSON `action_execution` does not override child JSON.
- A hard failure in one top-level action does not prevent later eligible top-level actions from running.
- Cargo AI aggregates top-level hard failures after all eligible actions finish.
- Cargo AI prints one run-level header, `Action execution: sequential` or `Action execution: parallel`, before action work begins.
- In redirected, piped, CI, or simpler terminal output, Cargo AI prefixes parent-visible action output with deterministic labels such as `[Action 1: generate_images]`.
- In append-only output, long-running steps also emit a step-start liveness line such as `step 2/2 generate_image started; waiting for provider response...`.
- Terminal lane summaries and the final run footer also include wall-clock durations, for example `completed in 31s.` and `✅ Run complete in 32s.`.
- When attached directly to an interactive terminal, Cargo AI switches to a compact live lane dashboard instead of append-only lifecycle lines.
- The first live dashboard slice keeps each lane block compact:
  - lane label
  - lane status with elapsed time while running and after terminal completion/failure
  - terminal step marker or current step when known
  - last lifecycle message
- In append-only output, parent-run `exec` and `email_me` output is emitted with the originating action label.
- Child-agent steps stay minimal in the parent lane with start/completion or exit-summary lines instead of recursively inlining the child transcript.

## Supported Step Kinds

These documented step kinds and helper fields are exhaustive for this contract.

Media actions require a compatible model and API project with access to the selected capability. Mistral image generation and speech remain unverified; its transcription route has been exercised. Existing OpenAI and Ollama image behavior is unchanged.

- `exec`
  - Required: `kind`, `program`, `args`
- `agent`
  - Required: `kind`, exactly one of `artifact` or legacy alias `agent`
  - Optional: `profile`, `usage_log`, `inputs`, `input_mode`, `input_overrides`, `run_vars`, `ignore_tools`
- `tool`
  - Required: `kind`, `name`
  - Optional: `params`, `output_variable`
  - Use this for Cargo AI-managed project-local tools created with `cargo ai add tool <name>`
  - `params` values may be JSON literals, including bounded arrays/objects, or exact single-key `{ "var": "<name>" }` references; other objects remain literal data
  - Literal params are checked against the tool `describe.params` contract during validation; variable params are checked after resolution at runtime
  - The tool `describe.result` schema must be a nullable string; if `output_variable` is set, the actual `invoke` result must be a non-null string
  - Every `invoke` response must contain exactly `protocol_version` and `result`; `result` must be present even when it is null
- `email_me`
  - Required: `kind`, `subject`, `text`
- `generate_image`
  - Required: `kind`, `prompt`, `path`
  - Optional: `model`, `profile`, `reference_images`
  - OpenAI image transport (API key or account), Gemini, Mistral, xAI, and Ollama's experimental image transport have provider-specific formats and reference limits.
  - If `model` is omitted, Cargo AI falls back to the effective invocation model resolved from the current profile and any `--model` CLI override.
  - If `profile` is present, Cargo AI resolves that profile at step runtime and uses it for the image step's provider/url/token context.
  - With `generate_image.profile`, explicit `model` still wins, then the step-profile model, then the parent invocation model.
  - `generate_image.profile` may switch providers; for example, a parent may stay on OpenAI while one image step uses an Ollama profile.
  - Anthropic and TypeSafe do not implement `generate_image`; select a compatible step profile when using either as the parent.
  - If neither the step nor the invocation provides a model, the step fails clearly at runtime.
  - `model` may be:
    - a literal non-empty string
    - a single variable reference such as `{ "var": "runtime.hero_image_model" }`
    - a single top-level string output field such as `{ "var": "image_model" }`
  - `generate_image.model` may not read captured `output_variable`, `status_variable`, or `error_variable` values
  - Use a tool-capable mainline model such as `gpt-5.2` for OpenAI account transport
  - For a direct OpenAI API token and URL, prefer GPT Image models such as `gpt-image-2`, `gpt-image-1.5`, or `gpt-image-1-mini`
  - `reference_images` may contain one or more `{ "input": "<named image input>" }` or `{ "path": "./assets/reference.png" }` entries
  - `reference_images[*].input` must point to a declared top-level input with `type: "image"`
  - `reference_images[*].path` must stay at the current level or below; absolute paths and `..` traversal are rejected
  - `reference_images` order is preserved; put the primary source/edit-target image first, then supporting detail, style, color, or material references
  - When using more than one reference image, label the roles in the prompt, such as "Image 1 is the source photo; Images 2 and 3 are style references only."
  - OpenAI API-key profiles send `reference_images` through the OpenAI image edit/reference-image path; OpenAI account profiles include them as Responses image input parts
  - For Ollama's experimental OpenAI-compatible `/v1/images/generations` endpoint, use an Ollama image model on an Ollama profile such as `x/flux2-klein:4b`
  - The current Ollama compatibility slice uses Ollama's documented `b64_json` response path, so Ollama-backed `generate_image` steps currently require a `.png` output path and do not support `reference_images`
  - Gemini writes `.jpg` or `.jpeg`; it accepts up to four PNG/JPEG references. xAI writes `.jpg` or `.jpeg`; it accepts up to five PNG/JPEG references. Each reference is at most 10 MiB and all references together at most 20 MiB. Mistral writes `.png` and does not accept references. New adapter responses and retrieved images are capped at 20 MiB; invalid or multiple images fail.
  - Mistral uses its image-generation tool, which creates a provider-hosted file before Cargo AI saves the local image. The tool can return no image; that is a failure, not a text-only success.
  - Current-at-ship-date note: official OpenAI docs list `gpt-image-2` for image generation and editing, including high-fidelity image inputs. Verified: 2026-05-22.

  Example reference-image step:

  ```json
  {
    "kind": "generate_image",
    "profile": "openai_api_key",
    "model": "gpt-image-2",
    "prompt": "Image 1 is the source photo. Image 2 is style reference only. Remove furniture from Image 1.",
    "reference_images": [
      { "input": "source_photo" },
      { "path": "./assets/style-reference.png" }
    ],
    "path": "./artifacts/edited.png"
  }
  ```

- `generate_audio` (speech from supplied text)
  - Required: `kind`, `text`, `voice`, `path`; optional: `profile`, `model` except for xAI's fixed speech service.
  - `text` and `path` accept a literal string or ordered string/variable parts. `voice` accepts a nonempty literal or one string variable reference. Voice IDs belong to the selected provider; there is no cross-provider voice mapping.
  - A model-selecting route uses explicit step `model`, then the selected step-profile model, then the invocation model. An incompatible inherited model fails; Cargo AI does not choose another speech model.
  - OpenAI API-key, Gemini, Mistral, and xAI API-key profiles are supported. Anthropic, Ollama, TypeSafe, and OpenAI account transport are unsupported for this step.
  - Use `.wav` for any supported provider or `.mp3` for OpenAI, Mistral, and xAI. Gemini speech outputs WAV only. Returned bytes must match the extension.
  - Mistral requires an existing saved voice ID accessible to the selected account. Cargo AI does not create, clone, or upload a voice. xAI accepts no step `model`; its fixed speech service uses the adapter's `language: auto` value.
  - Generated audio is limited to 20 MiB. A successful step writes one complete local file. The destination is staged and replaced only after generation and validation succeed; an existing file survives a provider or write failure.
- `transcribe_audio` (local speech file to text)
  - Required: `kind`, `audio: { "path": ... }`, `output_variable`; optional: `profile`, `model`.
  - `audio.path` is a literal relative path or a single `{ "var": "runtime.audio_path" }` reference to a string. It accepts `.wav` and `.mp3` files up to 10 MiB. It is a step-owned source, not a generic parent `file` input.
  - Its model precedence is explicit step `model`, selected step-profile model, then invocation model. The effective model must support the provider's native transcription route.
  - OpenAI API-key, Gemini, Mistral, and xAI API-key profiles use native transcription routes. Anthropic, Ollama, TypeSafe, and OpenAI account transport are unsupported.
  - A nonempty transcript is captured as text in `output_variable` for later steps in the same action. An empty or whitespace-only transcript fails without setting the capture. Transcription does not feed an earlier root inference pass.
  - For a transcript-to-child workflow, use an action-only coordinator with empty `agent_schema.properties`, then pass the captured variable to a text-compatible child `agent` step.

## Optional Control Fields

Every step allows its own kind-specific fields and the common controls below; extra fields are rejected in the strict revision. The JSON platform key is singular, `platform`.

- `when`
  - JSON Logic object evaluated by the parent action runner.
- `failure_mode`
  - Allowed values: `stop`, `continue`, `abort`
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
  - `exec`, `tool`, and `transcribe_audio` only
  - For `exec`, stores captured stdout.
  - For `tool`, stores the non-null string returned by the tool `invoke` response.
  - For `transcribe_audio`, stores the nonempty transcript text.

## Step Outcome Rules
- Steps stop the action by default when they fail.
- `failure_mode: "continue"` allows later steps to run after failure.
- `failure_mode: "abort"` stops scheduling new work for the current invocation, lets already-running work settle in the first slice, and fails the run with an explicit abort summary.
- A hard failure still stays local to that action's `run` list; later eligible top-level actions continue and the runtime aggregates top-level failures at the end.
- Child-agent abort stays local to the child invocation first; the parent lane then handles that failed child exit according to the parent step's own `failure_mode`.
- If a step is skipped because `when` is false, `status_variable` and `error_variable` stay unset in the MVP.
- If a step is skipped because `platform` does not match, `status_variable` and `error_variable` stay unset in the MVP.
- Matching steps still run in listed order.

## Returned Output vs Actions

- Top-level `agent_schema` fields are the returned output of the agent.
- Raw structured model output must pass declared shape, type, enum, numeric-bound and resource checks before any action starts; missing or unknown fields cause rejection.
- Action steps are side effects or follow-up orchestration after that output exists.
- `output_variable` captures step-local text from `exec`, a non-null string result from `tool`, or a nonempty transcript from `transcribe_audio`. It does not change the returned top-level output object.
- If `agent_schema.properties` is empty, Cargo AI skips the initial model call and starts directly at the action layer.
- In that structural action-only shape, top-level `inputs` are allowed only as named reusable parent-owned inputs.
- In that structural action-only shape, anonymous runtime `--input-*` flags remain invalid.
- Use `--input-override NAME=VALUE` to satisfy or replace declared named top-level inputs at invocation time.
- Validation establishes structural contracts, not the truth or trustworthiness of output. Model/tool output cannot add action steps or permissions; raw `exec` output remains text.

## Variable Namespace Rules
- Captured names are flat. Dotted names are invalid.
- `output_variable`, `status_variable`, and `error_variable` share one action-local namespace.
- `runtime` and `runtime.*` are reserved for declared invocation-scoped runtime variables.
- Captured names cannot collide with top-level `agent_schema` output field names.
- Captured names cannot be reused within the same action.
- The same captured names may be reused in different top-level actions.

## Variable Lookup Rules
- Top-level action `logic` can read:
  - top-level model output fields
  - declared `runtime.*` values
  - for the structural action-only shape, only declared `runtime.*` values are available at action start
- `when` and string-part substitutions can read:
  - top-level model output fields
  - declared `runtime.*` values
  - prior `output_variable` values from earlier steps in the same action
  - prior `status_variable` values from earlier steps in the same action
  - prior `error_variable` values from earlier steps in the same action
- Later actions cannot read captured variables from earlier top-level actions.

## Path Rules
- Local child agents use explicit same-level paths such as `./child_reporter`. Installed package exports may use `alias::entrypoint`; see [package workflow](package-workflow.md) for identity/version selection.
- Local file and image paths should stay relative.
- Parent-directory traversal such as `../` is invalid.
- `generate_image` output paths must use one of: `.png`, `.jpg`, `.jpeg`, `.webp`.
- For the current Ollama-backed `generate_image` compatibility path, use `.png`.
- `generate_audio` output paths use `.wav` or `.mp3`, subject to provider format support. `transcribe_audio.audio.path` uses a local `.wav` or `.mp3` source.
- Audio paths are portable relative paths without `..`. Packaged literal transcription sources resolve under declared package payload assets; packaged variable sources resolve under package data. Project variable sources resolve under its DataRoot. Other local sources resolve under the process working directory. The existing regular file is canonicalized inside its selected root before upload.
- Speech output uses package data for installed packages, project DataRoot when present, and otherwise the working directory.

## Child-Agent Data Flow

- Parent actions may pass child-agent `inputs`, including dynamic string parts resolved from current action-local data.
- Parent actions may also pass child-agent `run_vars` keyed by the intended child runtime var.
- Parent actions may also pass child-agent `input_overrides` keyed by the intended named child input.
- Parent actions may set child-agent `usage_log` to the same relative path they would pass with child `--usage-log <path>`.
- If the target is another Cargo AI agent, prefer a native `kind: "agent"` step instead of an `exec` wrapper that launches Python, shell, or another helper just to call the child.
- Use wrapper programs only when the task truly needs extra non-Cargo-AI behavior around that child call.
- Child `inputs` may also reference declared named top-level inputs with the exact shape `{ "input": "<name>" }`.
- Child `input_overrides` may also use `{ "input": "<name>" }` when the parent wants to bind one named parent input into one named child slot explicitly.
- Named child-input reuse is explicit only; parent inputs are not auto-inherited.
- Child `run_vars` values should stay CLI-shaped: string, number, boolean, or `{ "var": "<name>" }`.
- Child `input_overrides` values should stay CLI-shaped too: string, `{ "var": "<name>" }`, or `{ "input": "<name>" }`.
- Child `inputs` may still resolve declared `runtime.*` values alongside top-level model output fields and prior captured step variables.
- Parent `agent` steps should prefer `input_overrides` when targeting declared named child inputs and use child `inputs` for extra anonymous context.
- Parent `agent` steps should use child `run_vars` for invocation-scoped operational settings that the child already declares in top-level `runtime_vars`.
- Parent `agent` step `usage_log` values must be non-empty relative paths with no `..` traversal.
- Omitted child `usage_log` preserves current parent/root usage-log inheritance.
- Parent `agent` steps may set child `input_mode` to `replace`, `append`, or `prepend` when they also provide child `inputs`.
- Parent `agent` steps may also set a step-level `profile` as a literal string or single variable reference; Cargo AI resolves it at step runtime and forwards `--profile <name>` to the child.
- Child `input_mode` applies only to child `inputs`; it does not suppress or merge child `input_overrides`.
- Omitted child `input_mode` keeps the current replace behavior for child inputs.
- If a child wants to forward the same named input to its own child, it should declare that named top-level input locally first.
- Parent actions may capture child-agent success/failure with `status_variable` and `error_variable`.
- Parent actions cannot directly capture the child agent's top-level returned output fields into the parent action-local namespace.
- Cargo AI prints one root `using:` line at run start. In append-only output, it also emits another action-prefixed `using:` line when a child `agent` or `generate_image` step changes the effective `profile`, `auth`, `server`, or `model`.
- Interactive live mode keeps the parent dashboard at the orchestration level and does not surface child or step-level `using:` lines there.
- Native child-agent steps preserve child input forwarding, failure handling, depth limits, and runtime observability without inventing an extra scripting layer.

Example child usage log:

```json
{
  "kind": "agent",
  "agent": "./image_generator.json",
  "usage_log": "usage/image-generator-run.jsonl",
  "input_overrides": {
    "prompt": { "var": "image_prompt" }
  }
}
```

For installed package entrypoints, that relative `usage_log` path resolves under the alias `data/` root. Otherwise, an opted-in project uses its selected `[runtime] data_root`; without a selected project data root, local JSON and standalone hatched binaries retain the current run directory behavior. Explicit top-level usage-log destinations are not redirected. See [usage ledger](usage-ledger.md).

## Named Input Notes

- Top-level inputs may declare optional `name`.
- Prefer `name` when an input is part of the workflow contract, reusable by child steps, or intentionally operator-overrideable.
- For named input objects, prefer field order `name`, then `type`, then the value-bearing field.
- For unnamed literal inputs, keep `type` first and the value-bearing field second.
- Keep one-off root-model context unnamed when it does not need child reuse or targeted override behavior.
- Unnamed top-level inputs must keep a baked value.
- Named top-level inputs may keep a baked value or act as required slots with no baked value.
- Repeatable `--input-override NAME=VALUE` replaces declared named bindings for the current run.
- Anonymous runtime `--input-*` flags still control only the root model input list for schema-backed agents; they do not bind named input identities.

## Check Loop
1. Edit one JSON definition at a time.
2. Run `cargo ai hatch <agent-name> --config <config.json> --check`.
3. Fix validation errors before building.
4. Build only after `--check` passes.
