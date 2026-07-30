# Cargo AI Troubleshooting

Use this file when `cargo ai hatch <agent-name> --config <config.json> --check` reports an error.

## Common Problems

### Missing required top-level keys

Check that the JSON includes:
- `agent_definition_schema_version`
- `inputs`
- `agent_schema`
- `actions`

If the definition uses the legacy top-level `version` key, rename it to `agent_definition_schema_version` without changing its value. Copy schema-version values from current Cargo AI templates or guidance rather than inventing one from the current date or a package/project version.

### Wrong field for a step kind

Examples:
- `output_variable` on `agent` or `email_me`
- missing `program` for `exec`
- missing `agent` for `kind: "agent"`
- missing `name` for `kind: "tool"`
- unsupported or misspelled `platform` values

### Bad variable references

Check for:
- collisions with top-level schema fields
- reusing the same captured variable name twice in one action
- using a captured variable before the step that creates it

### Path problems

Check for:
- absolute paths
- `../`
- child agents that are not written as `./child_name`

### Runtime input confusion

Check for:
- using `--input-file`, `--input-url`, or `--input-image` without also supplying `--input-text` when the instructions were only baked into JSON and the run is still using the default replace behavior
- expecting runtime input flags to append to JSON `inputs` without also setting `--input-mode append` or `--input-mode prepend`
- using a JSON `file.path` when the caller should really choose the file at invocation time

### Named input confusion

Check for:
- declaring a named input slot with `name` plus `type` but no baked value, then never satisfying it before use
- expecting anonymous runtime `--input-*` flags to satisfy a named input slot; use `--input-override NAME=VALUE` for that
- using anonymous runtime `--input-*` on a structural action-only agent, where the model call is skipped and only named top-level inputs remain valid
- referencing `{ "input": "<name>" }` from a child step when the parent did not declare that named top-level input
- expecting a child to forward a named input onward without declaring the same named input locally first
- using child `inputs` when the intent was really to target one declared named child slot directly; prefer child `input_overrides` for that
- expecting child `input_mode` to change child `input_overrides`; it only applies to anonymous child `inputs`

### Runtime variable confusion

Check for:
- using `runtime.<name>` without declaring `<name>` in top-level `runtime_vars`
- passing `--run-var` for a name that is not declared in `runtime_vars`
- forgetting `--run-var` for a declared runtime var that has no `default`
- passing an empty `--run-var` value for `boolean`, `integer`, or `number`
- forgetting to quote a `--run-var` value that contains spaces or shell-sensitive characters
- trying to use captured step vars in `generate_image.model`; that field only accepts literal strings plus top-level string schema fields and `runtime.*` in the current contract

### Tool workflow confusion

Check for:
- trying to run `cargo ai add tool <name>` outside a Cargo AI project with no `.cargo-ai/project.toml`
- forgetting to bootstrap the project boundary first with `cargo ai init` or `cargo ai new <path>`
- updating `tools/<tool_name>/src/tool.rs` but forgetting `cargo ai tools build <tool_name> --target <triple>`
- pulling a published project and assuming source-backed tools are already materialized as runnable binaries
- wiring agent JSON to a tool `name` that does not match the managed `tool.json`
- using param names or scalar types that do not match the tool `describe` contract
- setting `output_variable` on a tool step when the tool returns `result: null`
- expecting `--ignore-tools` to make a missing tool succeed; it only skips the upfront audit

### Project bootstrap confusion

Check for:
- expecting `cargo ai add guidance` or `cargo ai add tool` to create `.cargo-ai/project.toml`; use `cargo ai init` or `cargo ai new <path>` first
- expecting `cargo ai init/new` to install the Codex guidance bundle automatically; run `cargo ai add guidance --style codex` after bootstrap when you want `AGENTS.md` plus `.cargo-ai/guidance/`
- forgetting that `cargo ai init/new` defaults to `--vcs git`
- seeing a Git setup failure and missing the suggestion to either install Git or rerun with `--vcs none`

### Step did not run on the current OS

Check for:
- `platform` filtering the step out on the current runtime OS
- a macOS-only command being tested on another platform
- platform-specific assumptions that belong in sidecar notes

### Child-agent expectations

Check for:
- expecting parent actions to read child top-level output fields directly
- missing `status_variable` / `error_variable` when the parent needs to react to child success or failure
- expecting the parent to inline the entire child transcript; Cargo AI only surfaces child start/completion summaries plus any changed child `using:` line by default

### Runtime observability confusion

Check for:
- expecting a repeated `using:` line when the effective `profile`, `auth`, `server`, and `model` did not change from the last printed context
- expecting `url=...` to appear for the standard OpenAI API or ChatGPT account transports; it only appears when the effective URL is custom or materially different
- assuming a child inherited the same context just because the parent emitted `child: started ...`; if the child changed context, look for a later child `using:` line

### Anthropic provider confusion

Check for:
- using a Claude.ai consumer subscription as if it supplied Console API credits or an API key; Anthropic bills Console API usage separately
- selecting `server = "anthropic"` with `auth = "none"` or `auth = "openai_account"`; use `auth = "api_key"`
- placing a real key in agent JSON or a command argument; store it with `cargo ai profile set <name> --stdin`
- using a model ID that the selected Anthropic Console organization cannot access
- pointing a custom URL at an OpenAI-compatible facade; Cargo AI's `anthropic` adapter expects the native Messages request and response contract
- sending direct file input or selecting an Anthropic profile for `generate_image`; use text, URL-text, or image input, or select an OpenAI/Ollama step profile for image generation
- assuming Cargo AI silently simplifies an unsupported JSON Schema; provider schema errors are surfaced so the authored contract remains visible

### Portability drift

If the user asked for portability across macOS, Windows, and Linux:
- remove shell-specific assumptions when possible
- minimize `exec` usage
- keep commands and paths as generic as possible

If the user asked for macOS-only local behavior:
- prefer `/bin/echo` or another explicit executable over a bare shell builtin
- pair the step with `platform: "macos"` when the step should not run elsewhere

## Default Fix Loop

1. Read the reported field path.
2. Fix one problem at a time.
3. Re-run:
   - `cargo ai hatch <agent-name> --config <config.json> --check`
4. Build only after the check passes.

## When To Add Sidecar Notes

If the JSON is technically valid but hard to explain:
- add a same-name sidecar Markdown file
- add an ASCII diagram near the top
- record any platform-specific assumptions there
