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
- `generate_audio` speaks supplied text into one local WAV or MP3 file through a supported provider/profile.
- `transcribe_audio` sends one local WAV or MP3 file to a transcription provider and captures text for later steps in that action.

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
- `output_variable` stores `exec` stdout, a non-null string result from `tool`, or nonempty transcript text from `transcribe_audio`

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

Child `agent`, `generate_image`, `generate_audio`, and `transcribe_audio` steps may select a step-level `profile`. The child receives that resolved profile as its runtime profile.

For provider-backed media steps with model selection, precedence is:

1. explicit step `model`
2. model from the step-level profile
3. effective model from the parent invocation

The step fails rather than guessing if none is available. Image models can be literals, declared runtime strings, or top-level string output fields; they cannot read captured step variables. xAI `generate_audio` is a fixed service: it skips inherited model resolution and rejects an explicit step `model`.

Keep local file, image, child, and output paths relative and at the current level or below. Parent traversal (`..`) is rejected. Local child targets should use explicit same-level paths such as `./child_reporter` or `./child_reporter.json`. Installed package exports may use `alias::entrypoint`; see [package identity and selection](./packages.md). Image output supports `.png`, `.jpg`, `.jpeg`, and `.webp`, subject to the selected provider's narrower limits.

Reference images may use a declared named image input (`{ "input": "source_photo" }`) or a definition-owned relative path. Their order is preserved; label each role in the prompt. Unsupported providers fail clearly instead of dropping the references or silently switching transports. See [Provider Setup](./providers/README.md) for provider capability boundaries.

Media actions require compatible models and API access for the selected capability. Mistral image generation and speech remain unverified; its transcription route has been exercised. See the provider guides for format and access limits.

For `generate_audio`, supply `text`, a provider-specific `voice`, and a relative output `path`. WAV works with OpenAI API-key, Gemini, Mistral, and xAI API-key profiles; MP3 also works with OpenAI, Mistral, and xAI. Gemini supports WAV only. An existing saved Mistral voice ID must already be accessible through the selected account. Generated audio is limited to 20 MiB. Cargo AI checks the returned container and replaces the output only after a complete valid file is staged, so a failed request preserves an existing file.

For `transcribe_audio`, set `audio.path` to a literal relative path or one string variable reference, and set `output_variable` to the transcript capture name. The source must be a local WAV or MP3 file no larger than 10 MiB. It is a run-step source, separate from generic model-facing `file` input. Cargo AI canonicalizes the existing regular file under the selected root before upload: package literals use declared payload assets, package variable paths use package data, project variable paths use its DataRoot, and ordinary local paths use the working directory. Speech output uses package data, project DataRoot when present, or the working directory. Portable relative paths with no parent traversal are required.

OpenAI API-key, Gemini, Mistral, and xAI API-key profiles have native speech and transcription routes. OpenAI account transport, Anthropic, Ollama, and TypeSafe are unsupported for these two steps. A successful transcription puts nonempty text into the action-local variable for later steps; an empty transcript fails without a capture. The root inference pass happens before actions, so use an action-only coordinator when the transcript must reach a child agent:

```json
{
  "agent_definition_schema_version": "2026-09-09.r1",
  "inputs": [],
  "agent_schema": { "type": "object", "properties": {} },
  "runtime_vars": { "audio_path": { "type": "string" } },
  "actions": [{
    "name": "transcribe_and_analyze",
    "logic": { "==": [1, 1] },
    "run": [{
      "kind": "transcribe_audio",
      "profile": "speech-transcription",
      "audio": { "path": { "var": "runtime.audio_path" } },
      "output_variable": "transcript"
    }, {
      "kind": "agent",
      "artifact": "./text-analyst.json",
      "profile": "text-analysis",
      "inputs": [{ "type": "text", "text": ["Analyze this transcript:\n", { "var": "transcript" }] }],
      "input_mode": "append"
    }]
  }]
}
```

Run it with `--run-var audio_path=./recordings/meeting.wav` after creating the two named compatible profiles and the child definition. A packaged literal recording must be explicitly declared as a package asset; runtime-variable recordings belong in runtime data, not the package payload. Avoid putting credentials or mutable recordings in package assets.

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

Child-agent `usage_log` must be a non-empty relative path without `..`. Omit it to keep the root usage log. For installed package entrypoints, a relative child log resolves under that package alias's persistent `data/` root. Otherwise, an opted-in project uses its selected `[runtime] data_root`; without a selected project data root, local JSON and standalone binaries retain the current run directory behavior. This does not redirect an explicit top-level usage-log destination.

## Choose Render Behavior

Cargo AI prints one root `using:` line with the effective profile, authentication mode, server, and model. A custom endpoint is shown as its origin (scheme, host and optional port), with credentials, path, query and fragment omitted. A changed child or image-step context produces an action-prefixed `using:` line in append-only output.

Select rendering with `--render-mode auto|live|append-only`:

- `auto` uses the terminal-sensitive default
- `live` requests the compact interactive action dashboard and falls back to append-only when unsupported
- `append-only` emits deterministic labeled lifecycle lines suited to logs, pipes, and CI

Append-only output labels action-owned output, reports liveness for long-running steps, and includes lane and run durations. Live mode stays at the parent orchestration level. Child steps show compact start/completion or exit summaries rather than recursively inlining child transcripts.

## Record Usage And Timing

Usage metadata is collected automatically in the selected Cargo AI Home for current interpreted/generated runtimes and their children. Query it with `cargo ai usage runs --json`, `usage show <run-id> --json`, or `usage summary --json`. Disable collection with `cargo ai usage settings --tracking off` or `CARGO_AI_USAGE_TRACKING=off`; existing history remains.

Request an additional per-run NDJSON export with `--usage-log <path>` or `CARGO_AI_USAGE_LOG=<path>`:

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
