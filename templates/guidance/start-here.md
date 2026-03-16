# Cargo AI Start Here

Use this file when a user opens a blank folder and wants help creating their first Cargo AI agent that hatches into a CLI tool.

## First-Run Goal

Do not start by asking the user to choose between a single agent, an action agent, or a parent/child setup.

Start by asking:

1. What do you want this agent to do?
2. What inputs should it accept?
3. What output fields should it return?
4. Does it need files, local commands, email, or another agent?
5. Near the end: where should it run?
   - on your current machine
   - or portable across macOS, Windows, and Linux

If the runtime exposes the current OS, treat that as a hint for the local-machine option, but still confirm whether the user wants local-only behavior or cross-platform portability.

When files, URLs, or images are involved, also clarify:
- should this content be baked into the JSON as a fixed default?
- or should the caller supply it at runtime with flags such as `--input-file` or `--input-url`?

## How To Drive The Conversation

- Keep the questions in plain language.
- Ask only for information needed to draft the first JSON.
- Explain the inferred pattern in plain language after you have enough detail.
- Prefer examples over abstract explanations.

## What To Do Next

1. Use `pattern-selection.md` to infer the right Cargo AI shape.
2. Copy the closest example from `examples/`.
3. Decide which inputs are baked into JSON and which will be supplied at runtime.
4. Draft the JSON in canonical field order.
5. If the flow becomes complex, recommend a same-name sidecar Markdown file.
6. Validate with:
   - `cargo ai hatch <agent-name> --config <config.json> --check`
7. Fix reported errors before building.

## Behavioral Defaults

- Prefer the most minimal portable approach that satisfies the user's goal.
- Use platform-specific steps only when the user explicitly wants local-machine behavior or the task cannot be met portably.
- Remember that runtime input flags replace the full baked `inputs` array unless the caller sets `--input-mode append` or `--input-mode prepend`. If a run still needs text instructions plus a runtime file, URL, or image in replace mode, supply both kinds of runtime inputs.
- Prefer boxed ASCII diagrams for explanations by default.
- If Mermaid rendering is clearly supported, offer it as an option and ask whether the user wants it.
