# Cargo AI Authoring Patterns

Use this file for human-readable structure and formatting guidance after you know what the agent should do.

## Canonical Field Order

Keep top-level keys in this order:

1. `version`
2. `inputs`
3. `agent_schema`
4. `actions`

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

## Naming Patterns

- Use short, behavior-based action names such as `send_summary` or `notify_on_failure`.
- Use explicit captured-variable names such as `child_status` or `report_error`.
- Do not overload one variable name for multiple meanings.

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

When the user wants portability across macOS, Windows, and Linux:
- avoid shell-specific scripts when possible
- avoid OS-specific paths
- keep file paths relative
- isolate any unavoidable platform-specific logic and explain it in the sidecar notes

When the user wants local-machine behavior:
- you may use local commands and conventions
- still keep the JSON as small and explicit as possible

## Validation Rhythm

- Draft the JSON.
- If complexity grows, add the sidecar Markdown file.
- Run `cargo ai hatch <agent-name> --config <config.json> --check`.
- Fix errors before building.
