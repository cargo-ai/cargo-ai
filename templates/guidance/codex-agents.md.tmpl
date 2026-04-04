# AGENTS.md

## Cargo AI Agent Authoring (Codex)

Use this workspace to create and maintain Cargo AI agent definition JSON files. Assume you do not have direct access to Cargo AI source code. The local guidance bundle in `.cargo-ai/guidance/` is the source of truth for authoring behavior, examples, and validation workflow.
Each JSON definition is meant to hatch into a CLI tool, so keep the behavior explicit and readable.

## Start Here

First, calibrate how much help the user wants:
- If unclear, ask whether they want step-by-step guidance or already know the schema they want.
- If the request is vague, guide them by eliciting:
  - inputs
  - `agent_schema`
  - actions
  - portability expectations
- Prefer the smallest correct readable agent that solves the task. Do not overbuild the JSON.

If no agent JSON exists yet:

1. Ask what the user wants the agent to do.
2. Ask what inputs it should accept.
3. Ask what outputs it should return.
4. Ask whether it needs files, local commands, email, or another agent.
5. Near the end, ask where it should run:
   - on the user's current machine
   - or portable across macOS, Windows, and Linux
6. Ask whether the caller should control any behavior at runtime, such as gating an action, choosing a threshold, or selecting an image model.
7. If the runtime exposes the current OS, treat that as a hint only. Still confirm whether the user wants local-only behavior or cross-platform portability.
8. Infer the right Cargo AI pattern using `.cargo-ai/guidance/pattern-selection.md`.
9. Do not ask the user to choose between "single agent", "action agent", or "parent/child" unless the user already understands those terms.
10. Draft from the closest example in `.cargo-ai/guidance/examples/`.
11. Validate with `cargo ai hatch <agent-name> --config <config.json> --check`.

## Local Guidance Bundle

Read these files before inventing structure:
- `.cargo-ai/guidance/start-here.md`
- `.cargo-ai/guidance/pattern-selection.md`
- `.cargo-ai/guidance/agent-definition-contract.md`
- `.cargo-ai/guidance/action-rules.md`
- `.cargo-ai/guidance/authoring-patterns.md`
- `.cargo-ai/guidance/examples/README.md`
- `.cargo-ai/guidance/troubleshooting.md`

Useful examples:
- `.cargo-ai/guidance/examples/basic-agent.json`
- `.cargo-ai/guidance/examples/schema-features.json`
- `.cargo-ai/guidance/examples/runtime-file-local-exec.json`
- `.cargo-ai/guidance/examples/child-agent.json`
- `.cargo-ai/guidance/examples/stop-by-default.json`
- `.cargo-ai/guidance/examples/continue-on-failure.json`
- `.cargo-ai/guidance/examples/conditional-when.json`
- `.cargo-ai/guidance/examples/runtime-vars-image-gating.json`

## Supplemental Public Context

Use these for positioning, onboarding flow, and public examples:
- public README: `https://github.com/analyzer1/cargo-ai#readme`
- website: `https://cargo-ai.org`

If these conflict with the local `.cargo-ai/guidance/` bundle, follow the local bundle for authoring behavior and validation rules.

## Working Defaults

- Prefer canonical `cargo ai hatch <agent-name> --config <config.json>` commands.
- Keep JSON strict. Do not add comments inside the executable `.json` file.
- Use canonical field order:
  - `version`
  - `inputs`
  - optional `action_execution`
  - optional `runtime_vars`
  - `agent_schema`
  - `actions`
- Within named input objects, prefer `name`, then `type`, then the value-bearing field. Keep unnamed literal inputs as `type` then value.
- The current supported output schema is flat and top-level only:
  - `string`
  - `number`
  - `integer`
  - `boolean`
- Current supported top-level schema metadata/constraints:
  - `description`
  - string-only `enum`
  - `minimum`, `maximum`, `exclusiveMinimum`, and `exclusiveMaximum` on `number` and `integer`
- Treat top-level arrays, nested objects, and union types as unsupported unless the local contract docs say otherwise.
- Treat the local contract and action helper docs as authoritative for step fields such as `platform`, runtime input override behavior, and child-agent data-flow limits.
- For `type: "url"`, prefer content Cargo AI can fetch directly with a normal HTTP request; treat `curl`-comparable ordinary pages as the target floor, not browser-only or JavaScript-rendered pages.
- Prefer the most minimal portable steps that satisfy the user's goal.
- Use platform-specific commands only when the user explicitly wants local-machine behavior or the task cannot be met portably.
- Remember that runtime input flags replace the full baked `inputs` array by default; use `--input-mode append` or `--input-mode prepend` when the caller should keep baked inputs too.
- Prefer named top-level `inputs` when one declared value is part of the workflow contract, reusable by child-agent steps, or overrideable by name at runtime.
- Keep one-off root-model context unnamed when it does not need that extra reusable identity.
- Reuse a declared named top-level input inside child `agent.inputs` with `{ "input": "<name>" }`.
- Prefer child `input_overrides` when the parent is targeting one declared named child input directly; keep child `inputs` for extra anonymous child context and use child `input_mode` exactly the same way the CLI uses `--input-mode`.
- Use child `run_vars` for the child's declared invocation-scoped settings and keep them CLI-shaped: string, number, boolean, or `{ "var": "..." }`.
- Keep child `input_overrides` CLI-shaped too: direct string literals, `{ "var": "..." }`, or `{ "input": "<name>" }` when forwarding a named parent input.
- If another Cargo AI agent should do the work, prefer a native `kind: "agent"` step. Do not create Python or shell wrappers just to launch the child agent.
- Use wrapper programs only when the task genuinely needs extra non-Cargo-AI behavior around that child invocation.
- Expect Cargo AI runtime output to print one root `using:` line at run start and another step-level `using:` line only when a child `agent` or `generate_image` step changes the effective `profile`, `auth`, `server`, or `model`.
- If `agent_schema.properties` is empty, Cargo AI uses the structural action-only shape: named top-level inputs are still allowed there, but anonymous runtime `--input-*` flags remain invalid and invocation-time changes should go through `--input-override NAME=VALUE`.
- Use top-level `runtime_vars` when the caller should control action behavior or step-local settings at invocation time without editing the JSON.
- Reference declared runtime vars as `runtime.<name>` and pass them with repeatable `--run-var name=value` flags.
- If a `--run-var` value contains spaces or shell-sensitive characters, quote it normally for the caller's shell.
- For `generate_image.model`, prefer a runtime-backed string such as `{ "var": "runtime.hero_image_model" }` when the operator should choose the image model at invocation time.
- Recommend a same-name sidecar Markdown file when the JSON becomes structurally complex.
- Prefer boxed ASCII diagrams for flow explanations.
- If Mermaid rendering is clearly supported, offer Mermaid as an option and ask whether the user wants it.
- Do not use `cargo ai new` or `cargo ai init` unless the user explicitly asks for those hidden flows.

## Validation Loop

1. Create or update one JSON definition at a time.
2. Keep the definition local in the current working directory unless the user asks for another location.
3. Validate first:
   - `cargo ai hatch <agent-name> --config <config.json> --check`
4. Fix schema, action, or contract errors before building.
5. Build only after `--check` passes:
   - `cargo ai hatch <agent-name> --config <config.json>`
6. Rebuild with `--force` only when replacing an existing output is intentional.

## Editing Guidance

- Adapt to the user's familiarity. Guide a new user step by step; move faster when the user already knows the structure they want.
- Make the smallest JSON change that satisfies the request.
- Explain inferred structure in plain language when the user is new to Cargo AI.
- When adding a new field or action, preserve surrounding structure and naming patterns.
- If the user asks for a new agent from scratch, start from the closest local example instead of inventing a shape from memory.
- Summarize validation failures concretely and then fix the JSON.
- After updating a definition, explain what changed in plain language and summarize what `hatch --check` proved.
