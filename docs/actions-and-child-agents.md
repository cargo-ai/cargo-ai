# Build Actions And Child-Agent Workflows

[Documentation hub](./README.md) · [Project README](../README.md)

Actions run after Cargo AI has validated the top-level model output. Use them for bounded side effects and orchestration: invoke a local program, call a project tool, send email, create an image, or hand work to another Cargo AI agent.

This page explains the workflow and runtime boundaries. The generated [action rules](../templates/guidance/action-rules.md) and [agent definition contract](../templates/guidance/agent-definition-contract.md) are the version-matched offline assistant references for action validation.

## Add One Action First

Each action has a `name`, a JSON Logic `logic` condition, and an ordered `run` list. At action start, `logic` can read top-level model output and declared `runtime.*` values. A structural action-only agent has no model output, so its action logic begins with runtime values only.

```json
{
  "actions": [
    {
      "name": "save_review",
      "logic": {
        "==": [{ "var": "needs_review" }, true]
      },
      "run": [
        {
          "kind": "exec",
          "program": "./save_review",
          "args": [{ "var": "summary" }]
        }
      ]
    }
  ]
}
```

Action `logic` uses [JSON Logic](https://jsonlogic.com/). Keep an action narrow enough that its condition, side effects, and failure policy remain obvious during review.

## Choose A Run-Step Kind

Cargo AI supports these step kinds:

- `exec` runs a local executable with arguments.
- `agent` invokes a child Cargo AI executable, JSON definition, or declared package entrypoint.
- `tool` invokes a Cargo AI-managed project-local tool.
- `email_me` sends an account-backed email action.
- `generate_image` writes one local image artifact through a supported provider/profile.

Each kind has a different required field set and substitution boundary. Use the [action rules](../templates/guidance/action-rules.md) instead of copying a field matrix into your project. For tool creation and parameter contracts, read [Projects And Local Tools](./projects-and-tools.md).

## Control Scheduling

Matching top-level actions run sequentially by default. Add top-level `"action_execution": "parallel"` to allow them to overlap. Parallel mode changes only scheduling across top-level actions; every action's own `run` steps stay in listed order.

For a safety or test invocation, `--action-execution sequential` forces a parallel-capable invocation tree to run sequentially. The override is invocation-scoped and inherited by child agents; a parent's JSON setting does not replace a child's JSON setting.

A non-abort hard failure stays local to that action's remaining step list. Other eligible top-level actions still run, and Cargo AI aggregates their failures when action work finishes.

## Handle Step Outcomes Deliberately

Steps use `failure_mode: "stop"` by default:

- `stop` ends the current action's remaining steps
- `continue` records the failure and lets later steps in that action run
- `abort` stops scheduling new work for the invocation, lets already-running work settle, and fails with an abort summary

A child abort first ends the child invocation. The parent then handles the failed child step according to the parent's own `failure_mode`.

Use optional outcome variables when a later step must react:

- `status_variable` stores `succeeded` or `failed`
- `error_variable` stores a human-readable failure
- `output_variable` stores `exec` stdout or a non-null string result from `tool`

```json
{
  "run": [
    {
      "kind": "exec",
      "program": "./save_review",
      "args": [{ "var": "summary" }],
      "output_variable": "saved_path",
      "status_variable": "save_status",
      "error_variable": "save_error",
      "failure_mode": "continue"
    },
    {
      "kind": "email_me",
      "when": { "==": [{ "var": "save_status" }, "succeeded"] },
      "subject": "Review saved",
      "text": ["Saved to ", { "var": "saved_path" }]
    }
  ]
}
```

`when` uses JSON Logic and may read model output, declared runtime values, and variables captured by earlier steps in the same action. Captured names are flat, cannot collide with top-level output fields, and cannot be reused within one action. Later top-level actions cannot read them.

When `when` is false or `platform` filters a step out, the step is skipped and its status/error variables remain unset. An `output_variable` is action-local follow-up data; it does not change the agent's returned top-level object.

## Select Runtime Platforms

Use `platform` only for genuinely platform-specific work. It accepts `macos`, `linux`, or `windows`, either as one string or an array:

```json
{
  "kind": "exec",
  "program": "./save_report.sh",
  "platform": ["macos", "linux"],
  "args": [{ "var": "summary" }]
}
```

An omitted platform makes the step eligible everywhere. Prefer portable tools and executables when the workflow is expected to work across operating systems.

## Resolve Profiles, Models, And Paths

Child `agent` and `generate_image` steps may select a step-level `profile`. The child receives that resolved profile as its runtime profile.

For `generate_image`, model precedence is:

1. explicit step `model`
2. model from the step-level profile
3. effective model from the parent invocation

The image step fails rather than guessing if none is available. Its model can be a literal, a declared runtime string, or a top-level string output field; it cannot read a captured step variable.

Keep local file, image, child, and output paths relative and at the current level or below. Parent traversal (`..`) is rejected. Child targets should use explicit same-level paths such as `./child_reporter` or `./child_reporter.json`. Image output supports `.png`, `.jpg`, `.jpeg`, and `.webp`, subject to the selected provider's narrower limits.

Reference images may use a declared named image input (`{ "input": "source_photo" }`) or a definition-owned relative path. Their order is preserved; label each role in the prompt. Unsupported providers fail clearly instead of dropping the references or silently switching transports. See [Provider Setup](./providers/README.md) for provider capability boundaries.

Package child paths and permissions have additional rules. See [Packages](./packages.md) before invoking an `alias::entrypoint` target.

## Hand Work To A Child Agent

Use a native `kind: "agent"` step when the target is another Cargo AI agent. Use a Python or shell wrapper only when it adds behavior beyond launching that child.

Prefer `artifact` for the child target. Cargo AI still accepts the legacy `agent` field, but new definitions should use `artifact`; never set both.

```json
{
  "kind": "agent",
  "artifact": "./child_reporter.json",
  "profile": { "var": "runtime.child_profile" },
  "usage_log": "usage/child-reporter.jsonl",
  "run_vars": {
    "review_year": { "var": "runtime.review_year" }
  },
  "input_overrides": {
    "source_report": { "input": "source_report" },
    "review_reason": { "var": "summary" }
  },
  "input_mode": "append",
  "inputs": [
    {
      "type": "text",
      "text": "Prepare a concise follow-up."
    }
  ],
  "status_variable": "child_status",
  "error_variable": "child_error"
}
```

The four child input surfaces mirror the generated CLI:

- `run_vars` passes values for the child's declared `runtime_vars`
- `input_overrides` targets declared named child inputs
- `inputs` supplies anonymous runtime input
- `input_mode` controls only anonymous child inputs (`replace`, `append`, or `prepend`)

Named inputs are never inherited automatically. The parent must declare a named top-level input before using `{ "input": "<name>" }`, and a middle agent must declare the same name locally before forwarding it again. Prefer `input_overrides` for a named child slot and `inputs` for extra anonymous context.

Child `run_vars` accept strings, numbers, booleans, or variable references. Child `input_overrides` accept strings, variable references, or references to named parent inputs. Consult the [agent definition contract](../templates/guidance/agent-definition-contract.md) for the complete accepted shapes.

## Respect Child Boundaries

By default, a root and its descendants share these safety limits:

- maximum child-agent depth: `5`, overridden by `--max-agent-depth`
- total runtime budget: `600` seconds, overridden by `--max-runtime-in-sec`

A parent may capture whether a child succeeded or failed, but it cannot automatically merge or read the child's structured top-level result. Design an explicit external artifact or tool-mediated handoff if a later parent step needs child-produced data.

Child-agent `usage_log` must be a non-empty relative path without `..`. Omit it to keep the root usage log. For installed package entrypoints, a relative child log resolves under that package alias's persistent `data/` root; for local JSON or standalone binaries, it resolves from the current run directory.

## Choose Render Behavior

Cargo AI prints one root `using:` line with the effective profile, authentication mode, server, and model. It includes a URL only when it is custom or materially different from the standard transport. A changed child or image-step context produces an action-prefixed `using:` line in append-only output.

Select rendering with `--render-mode auto|live|append-only`:

- `auto` uses the terminal-sensitive default
- `live` requests the compact interactive action dashboard and falls back to append-only when unsupported
- `append-only` emits deterministic labeled lifecycle lines suited to logs, pipes, and CI

Append-only output labels action-owned output, reports liveness for long-running steps, and includes lane and run durations. Live mode stays at the parent orchestration level. Child steps show compact start/completion or exit summaries rather than recursively inlining child transcripts.

## Record Usage And Timing

Opt into a usage ledger with `--usage-log <path>` or `CARGO_AI_USAGE_LOG=<path>`:

```bash
cargo ai run ./my_agent.json \
  --profile openai-account \
  --usage-log ./usage.ndjson
```

The file is newline-delimited JSON. It records metadata events for root runs, agent runs, provider requests, tool runs, and completion. When a provider reports usage, Cargo AI normalizes input, output, and total token counts; otherwise it records timing/status and leaves usage null rather than estimating.

`root_run_id` links the invocation tree. Each execution has its own `agent_run_id` and `parent_agent_run_id`, and `depth` helps reconstruct the hierarchy. Source metadata distinguishes local paths, inline/stdin sources, registry definitions, and hatched agents where available.

Usage logs do not contain prompts, model output, generated image bytes, tool arguments, tool stdout/stderr, profile tokens, access tokens, or raw provider responses. Implement a project tool when the workflow requires business logs or decision traces.

For the full event contract and interpretation guidance, read the generated [usage ledger reference](../templates/guidance/usage-ledger.md).

## Check The Workflow

Validate the definition before exporting it:

```bash
cargo ai hatch my_agent --config ./my_agent.json --check
```

Then run it with the intended input mode, runtime variables, profile, platform, action scheduling, render mode, and usage-log settings. Review every action as an executable permission boundary, especially `exec`, package-child, email, and provider-backed steps.

## Related Documentation

- [Documentation hub](./README.md)
- [Project README](../README.md)
- [Agent Definitions](./agent-definitions.md)
- [Projects And Local Tools](./projects-and-tools.md)
- [Packages](./packages.md)
- [Provider Setup](./providers/README.md)
- [Troubleshooting](./troubleshooting.md)
