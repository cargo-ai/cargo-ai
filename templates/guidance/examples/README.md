# Cargo AI Guidance Examples

Use these examples as starting points. Copy the closest file, rename it, and then edit only what the user needs.
Each example is meant to be validated and hatched into a CLI executable.

## Example Index

- `basic-agent.json`
  - Smallest useful starting point for a single agent with simple output and one straightforward action.
- `schema-features.json`
  - Shows the supported scalar schema surface together: `string`, `integer`, `number`, `boolean`, `description`, string `enum`, and numeric bounds, while still keeping the action flow small.
- `runtime-file-local-exec.json`
  - Shows a returned summary field, a definition-owned placeholder file input, and a macOS-only local echo step using `/bin/echo`.
- `child-agent.json`
  - Minimal child agent that can be called from a parent.
  - Good starting point when you need a parent to forward one named top-level input with `{ "input": "<name>" }`.
- `stop-by-default.json`
  - Shows default stop behavior when a later step should not continue after failure.
- `continue-on-failure.json`
  - Shows `failure_mode: "continue"` with `status_variable`, `error_variable`, and follow-up behavior.
- `conditional-when.json`
  - Shows step-level `when` gating and branching inside one action.
- `runtime-vars-image-gating.json`
  - Shows top-level `runtime_vars`, `--run-var`-driven action gating, typed runtime vars in JSON Logic, and runtime-backed `generate_image.model`.

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
- Use `runtime-vars-image-gating.json` when the main question is how caller-supplied typed runtime vars control action behavior or image-model selection.
- For named reusable parent inputs and child forwarding, start from `child-agent.json` plus the named-input examples in the main README. Use top-level named `inputs` when one value should be reusable by child steps or overrideable with `--input-override NAME=VALUE`.
- When a parent is filling one declared named child slot directly, prefer child `input_overrides`. Keep child `inputs` for extra anonymous context and use child `input_mode` only for that anonymous child-input list.
- When converting a baked file example into a caller-supplied runtime file workflow, remember that `--input-file` replaces the full baked `inputs` array unless the caller also sets `--input-mode append` or `--input-mode prepend`. Supply `--input-text` too when the run stays in replace mode and still needs text instructions.
- If the JSON becomes hard to scan, add a same-name sidecar Markdown file and an ASCII flow diagram.

## Recommended Smoke Checks

After `cargo ai hatch <agent-name> --config <config.json> --check` passes, prefer one small manual smoke loop for runtime-heavy definitions:

1. Run one invocation that supplies `--run-var` values and proves the expected action `logic` branch executes.
2. If the definition uses `when`, run one invocation where `when` should skip the step and one where it should pass.
3. If the definition uses `generate_image.model`, run one invocation that sets the image model through `--run-var` and confirm the step resolves the expected model string.
4. If the definition uses named top-level inputs, run one invocation with `--input-override NAME=VALUE` and confirm any child `{ "input": "<name>" }` forwarding resolves the overridden value.
5. If the definition includes a required named input slot with no baked value, run one failure case too and confirm the unresolved-slot error is clear.
6. If the definition calls a child agent with child `input_overrides`, run one invocation that proves the child receives the named override and one invocation that mixes child `input_overrides` with anonymous child `inputs`.
