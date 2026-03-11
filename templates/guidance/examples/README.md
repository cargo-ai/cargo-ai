# Cargo AI Guidance Examples

Use these examples as starting points. Copy the closest file, rename it, and then edit only what the user needs.

## Example Index

- `basic-agent.json`
  - Smallest useful starting point for a single agent with simple output and one straightforward action.
- `schema-features.json`
  - Shows the supported scalar schema surface together: `string`, `integer`, `number`, `boolean`, `description`, string `enum`, and numeric bounds, while still keeping the action flow small.
- `runtime-file-local-exec.json`
  - Shows a returned summary field, a definition-owned placeholder file input, and a macOS-only local echo step using `/bin/echo`.
- `child-agent.json`
  - Minimal child agent that can be called from a parent.
- `stop-by-default.json`
  - Shows default stop behavior when a later step should not continue after failure.
- `continue-on-failure.json`
  - Shows `failure_mode: "continue"` with `status_variable`, `error_variable`, and follow-up behavior.
- `conditional-when.json`
  - Shows step-level `when` gating and branching inside one action.

## How To Use The Examples

1. Read `start-here.md` and gather the user's goal.
2. Use `pattern-selection.md` to choose the closest example.
3. Copy the example into a working JSON file.
4. Preserve canonical field order while editing.
5. Run `cargo ai hatch <agent-name> --config <config.json> --check`.

## Notes

- These examples are teaching tools, not a forced template system.
- Prefer the smallest example that matches the user's goal.
- Keep using the action-flow examples for `when`, `failure_mode`, and captured-variable behavior. Use `schema-features.json` when the main question is how to express the supported output fields and constraints.
- Use `runtime-file-local-exec.json` when the main question is how file summarization, returned output, and a platform-specific local exec step fit together.
- When converting a baked file example into a caller-supplied runtime file workflow, remember that `--input-file` replaces the full baked `inputs` array. Supply `--input-text` too when the run still needs text instructions.
- If the JSON becomes hard to scan, add a same-name sidecar Markdown file and an ASCII flow diagram.
