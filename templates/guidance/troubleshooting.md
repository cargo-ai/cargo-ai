# Cargo AI Troubleshooting

Use this file when `cargo ai hatch <agent-name> --config <config.json> --check` reports an error.

## Common Problems

### Missing required top-level keys

Check that the JSON includes:
- `version`
- `inputs`
- `agent_schema`
- `actions`

### Wrong field for a step kind

Examples:
- `output_variable` on `agent` or `email_me`
- missing `program` for `exec`
- missing `agent` for `kind: "agent"`
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

### Runtime variable confusion

Check for:
- using `runtime.<name>` without declaring `<name>` in top-level `runtime_vars`
- passing `--run-var` for a name that is not declared in `runtime_vars`
- forgetting `--run-var` for a declared runtime var that has no `default`
- passing an empty `--run-var` value for `boolean`, `integer`, or `number`
- forgetting to quote a `--run-var` value that contains spaces or shell-sensitive characters
- trying to use captured step vars in `generate_image.model`; that field only accepts literal strings plus top-level string schema fields and `runtime.*` in the current contract

### Step did not run on the current OS

Check for:
- `platform` filtering the step out on the current runtime OS
- a macOS-only command being tested on another platform
- platform-specific assumptions that belong in sidecar notes

### Child-agent expectations

Check for:
- expecting parent actions to read child top-level output fields directly
- missing `status_variable` / `error_variable` when the parent needs to react to child success or failure

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
