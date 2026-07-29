# Cargo AI Authoring Patterns

Use this file for human-readable structure and formatting guidance after you know what the agent should do.
These patterns shape the JSON that drives the generated CLI tool.

## Canonical Field Order

Keep top-level keys in this order:

1. `agent_definition_schema_version`
2. `inputs`
3. optional `action_execution`
4. optional `runtime_vars`
5. `agent_schema`
6. `actions`

`agent_definition_schema_version` identifies the Cargo AI contract used to interpret the definition. It is not the agent or package version; package version is `[project].version`. Copy the value from the current Cargo AI template or guidance instead of inventing one from the current date, a project/package version, or the Cargo AI product version.

Keep each action in this order:

1. `name`
2. `logic`
3. `run`

Keep each run step in this order:

1. `kind`
2. required kind-specific fields
3. `when`
4. `failure_mode`
5. `output_variable`
6. `status_variable`
7. `error_variable`

Keep input objects easy to scan:

- named input objects: `name`, then `type`, then the value field such as `text`, `url`, or `path`
- unnamed literal inputs: `type`, then the value field
- `generate_image.reference_images`: source/edit-target image first, then supporting detail, style, color, or material references

## Naming Patterns

- Use short, behavior-based action names such as `send_summary` or `notify_on_failure`.
- Use explicit captured-variable names such as `child_status` or `report_error`.
- Do not overload one variable name for multiple meanings.

## Input and Runtime Strategy

Choose one of these intentionally:

- baked inputs
  - keep instructions and fixed local paths in the JSON `inputs` array
- runtime inputs
  - let the caller provide values such as `--input-text`, `--input-file`, `--input-url`, or `--input-image`

Important rule:
- If runtime input flags are used without `--input-mode`, they replace the full baked `inputs` array for that run.
- Use `--input-mode append` to keep baked inputs first, or `--input-mode prepend` to place runtime inputs first.
- If the caller supplies `--input-file` in replace mode but the agent also needs instructions, the caller should also supply `--input-text`.

Use baked file/image paths only when the definition should own a fixed local asset. Use runtime flags when the caller should choose the asset.

Use top-level `runtime_vars` when the caller should choose action behavior or a step-local setting without editing the JSON:

- gating flags such as `runtime.generate_images`
- thresholds such as `runtime.score_threshold`
- step-local image model selection such as `runtime.hero_image_model`

Reference them as `runtime.<name>` and pass them with `--run-var name=value`.

## Sidecar Notes

When a JSON definition becomes complex, recommend a same-name sidecar Markdown file:

- `my_agent.json`
- `my_agent.md`

Good sidecar sections:
- goal
- expected inputs
- expected outputs
- action flow
- child-agent relationships
- testing notes

## Diagram Guidance

Prefer boxed ASCII diagrams by default because they work in CLI sessions, editors, and diffs.

Example:

```text
+----------------------+
| Parent Agent         |
+----------+-----------+
           |
           v
+----------------------+
| when run_child=true  |
| agent: ./child_demo  |
+----------+-----------+
           |
     +-----+-----+
     |           |
     v           v
+-----------+  +-----------+
| succeeded |  | failed    |
+-----+-----+  +-----+-----+
      |                |
      v                v
+-----------+    +-----------+
| success   |    | failure   |
| branch    |    | branch    |
+-----------+    +-----------+
```

If the runtime clearly supports Mermaid rendering, offer Mermaid as an additional option and ask whether the user wants that representation. When support is uncertain, stay with ASCII only.

## Portability Guidance

Default to the most portable shape that satisfies the user's goal.

Prefer these in order:
1. plain model output with minimal actions
2. `email_me` or child-agent steps when they fit the task
3. `exec` only when the task truly needs a local command

If one Cargo AI agent needs to call another Cargo AI agent:
- prefer a native `kind: "agent"` step
- do not add Python or shell wrappers just to launch the child agent
- use wrapper programs only when the task truly needs non-Cargo-AI behavior around that call

If the user needs a reusable project-local capability:
- prefer a native `kind: "tool"` step over inventing repeated `exec` wrappers
- when Cargo is available and new executable code is needed, create a Rust tool with `cargo ai add tool <name>` instead of ad hoc Python, Node, or shell helper scripts
- put custom behavior in `src/tool.rs`; leave `src/lib.rs` focused on Cargo AI protocol adapter code unless the protocol contract itself must change
- when adding dependencies, follow the dependency discipline in `tool-hardening.md` instead of grabbing the first crate that compiles
- before completion, perform the hardening review in `tool-hardening.md` and treat the tool as production local executable code
- keep the tool contract in the tool crate and keep the agent JSON focused on `name`, `params`, and optional output capture
- use `.cargo-ai/guidance/tool-authoring.md` for the workflow, `.cargo-ai/guidance/tool-contract.md` for the detailed contract, and `.cargo-ai/guidance/tool-child-agents.md` when the tool needs child agents

When the user wants portability across macOS, Windows, and Linux:
- avoid shell-specific scripts when possible
- avoid OS-specific paths
- keep file paths relative
- isolate any unavoidable platform-specific logic and explain it in the sidecar notes

When the user wants local-machine behavior:
- you may use local commands and conventions
- prefer real executables over shell builtins when possible
- for macOS-only examples, prefer `/bin/echo` over bare `echo`
- pair intentionally platform-specific steps with `platform`
- still keep the JSON as small and explicit as possible

## Validation Rhythm

- Draft the JSON.
- If complexity grows, add the sidecar Markdown file.
- Run `cargo ai hatch <agent-name> --config <config.json> --check`.
- Fix errors before building.
