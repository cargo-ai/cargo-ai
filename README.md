# cargo-ai™

[![Audit Status](https://github.com/analyzer1/cargo-ai/actions/workflows/security-audit.yml/badge.svg)](https://github.com/analyzer1/cargo-ai/actions/workflows/security-audit.yml)
[![Multi-OS CI](https://github.com/analyzer1/cargo-ai/actions/workflows/multi-os-ci.yml/badge.svg)](https://github.com/analyzer1/cargo-ai/actions/workflows/multi-os-ci.yml)
[![Status: Stable – Ongoing Development](https://img.shields.io/badge/Status-Stable_–_Ongoing_Development-blue)](https://github.com/analyzer1/cargo-ai)

Build AI-powered CLI tools from a single JSON definition.

Define declarative agents in JSON, hatch native executables locally, and share them in minutes.

Cargo AI is an open-source CLI for building auditable AI-powered CLI tools from a single JSON definition. Define inputs, schema, and actions once, run the JSON directly with `cargo ai run --config`, or hatch a native executable with `cargo ai hatch`, then inspect, run, and share it on your terms.

```bash
cargo ai hatch agent_x
```

Cargo AI keeps agent behavior readable, auditable, and understandable through a single JSON definition.

## Why Cargo AI

- **Declarative by Design**: define exactly what the agent does, what actions it can take, and keep the behavior easy to inspect.
- **Open Source and Fully Auditable**: inspect the generated code, understand what ships, and keep control of the runtime.
- **Handles Real Inputs**: work with text, images, URLs, and common files.
- **Supports Advanced Logic**: add conditions and follow-up behavior without hand-building a custom app.
- **Real Actions, Not Just Prompts**: run local commands, call child agents, pass command-line arguments, and send email follow-ups.
- **Choose Your Own AI**: use OpenAI models today or open-source models through Ollama, with room for more providers over time.
- **You Own the Output**: hatch a local executable and generated code that you can keep, modify, and run wherever you want.
- **Portable Across macOS, Linux, and Windows**: keep one readable agent definition and hatch it for the systems you care about.
- **Easy to Share Through `cargo-ai.org`**: create a free account to publish definitions in minutes so other people can hatch them locally on their own machines.
- **No Extra Token Plumbing Required**: use your existing Codex workflow when it fits, or bring your own model access when you want direct provider control.
- **Built for AI-Assisted Iteration**: keep the agent readable, diffable, and easy to improve with tools like Codex.
- **Built to Grow With You**: start with one clear definition, then add commands, email actions, and shared definitions as your workflow expands.

A concise JSON definition keeps the agent easy to read, review, diff, and improve without losing trust in what it does.

## Quick Start

### 0. Install Cargo

Cargo AI requires Rust and Cargo. If you do not already have them, install Rust with `rustup` using the official guide for macOS, Linux, or Windows. This usually takes a few minutes.

Official install guide: [Install Rust](https://rust-lang.org/tools/install/)

After installation, verify Cargo is available:

```bash
cargo --version
```

### 1. Install `cargo-ai`

```bash
cargo install cargo-ai --locked
cargo ai --help
```

Cargo AI uses a product-oriented pre-`1.0.0` release policy: `0.y.0` means meaningful product/contract evolution, while `0.y.z` is reserved for smaller fixes and polish. See [VERSIONING.md](./VERSIONING.md) for the public versioning policy.

Upgrades remain manual via `cargo install cargo-ai --locked`. After a meaningful pre-`1.0.0` upgrade such as `0.1.0 -> 0.2.0`, generated agents may report that they are out of sync with local Cargo AI metadata until they are re-hatched.

By default, Cargo AI stores config, credentials, and internal workspaces under `~/.cargo/.cargo-ai` (or `$CARGO_HOME/.cargo-ai`). Set `CARGO_AI_HOME` if you want Cargo AI to use a different root directory.

### 2. Choose your model setup

**Option A: recommended if you use ChatGPT Plus or above**

Includes Codex at no additional cost. This is the easiest path today. `cargo-ai` uses your Codex login, so no separate API key is required.

```bash
codex login

cargo ai profile add openai-account \
  --server openai \
  --model gpt-5.3 \
  --auth openai_account \
  --default

cargo ai auth login openai --profile openai-account --set-default
```

If you do not already have Codex installed, get it here:
[Codex CLI setup](https://developers.openai.com/codex/cli)

**Option B: direct provider control**

Use this path if you want an explicit model profile with direct provider credentials and no Codex dependency.

```bash
cargo ai profile add openai \
  --server openai \
  --model gpt-5.3 \
  --auth api_key \
  --default

cargo ai profile set openai --token sk-*** --auth api_key
```

**Option C: open-source models with Ollama**

Use this path if you want to run `cargo-ai` without ChatGPT or OpenAI at all.

Install Ollama here:
[Get Ollama](https://ollama.com/download)

Then pull a model such as `mistral` and add a local profile:

```bash
ollama pull mistral

cargo ai profile add ollama \
  --server ollama \
  --model mistral \
  --default
```

### 3. Run a sample agent directly

```bash
cargo ai run adder_test --profile openai-account
```

You can also run a local definition with `cargo ai run ./adder_test.json` or
`cargo ai run --config ./adder_test.json`.

### 4. Hatch the same sample as a standalone executable

```bash
cargo ai hatch adder_test
./adder_test
```

On Windows, run `adder_test.exe` or just `adder_test`.

### 5. Register an account

Define agent email alerts with `cargo-ai.org` and manage your agents in one place. Keep them private, or share them instantly with anyone in the world.

```bash
cargo ai account register you@example.com
cargo ai account confirm <code-from-email>
```

Optional: set a custom public handle

If you want a specific public handle, set it here. Otherwise, `cargo-ai.org` assigns one automatically, and you can change it later.

```bash
cargo ai account handle --set your-handle
```

Once registered, you can push an agent definition to your account repository and hatch it locally:

```bash
cargo ai account agents push adder_test.json --name adder_test
cargo ai account agents hatch adder_test
```

## The Core Mental Model

> [!TIP]
> You do not need to author this by hand. The fastest path is to tell Codex exactly what kind of agent you want and let it update the file for you. Read this section so the structure is easy to recognize, then review the result and verify exactly what the agent does. When you're ready for that loop, jump to [Best First Workflow in Codex](#best-first-workflow-in-codex).

Cargo AI keeps the authoring model intentionally small:

1. optional `inputs`
   Ordered model-facing input such as `text`, `url`, or `image`.
2. optional `runtime_vars`
   Typed caller-supplied values that can control action logic, `when`, and selected run-step fields at invocation time.
3. `agent_schema`
   The typed response you expect back.
4. `actions`
   What to do after the response is validated, including the ordered `run` steps inside each action.

The next section expands those same pieces from minimal snippets into richer patterns.

A minimal agent looks like this:

```json
{
  "version": "2026-03-03.r1",
  "inputs": [
    {
      "type": "text",
      "text": "What is 2 + 2? Return the answer as an integer."
    }
  ],
  "agent_schema": {
    "type": "object",
    "properties": {
      "answer": {
        "type": "integer",
        "description": "The result of the math problem."
      }
    }
  },
  "actions": [
    {
      "name": "print_answer",
      "logic": { "==": [{ "var": "answer" }, 4] },
      "run": [
        {
          "kind": "exec",
          "program": "echo",
          "args": ["The answer is 4."]
        }
      ]
    }
  ]
}
```

That JSON can run directly through Cargo AI:

```bash
cargo ai run ./my_agent.json --profile openai-account
```

Or it can become a compiled local executable through:

```bash
cargo ai hatch my_agent --config ./my_agent.json
./my_agent
```

For Windows users, run `my_agent.exe` or just `my_agent`.

You can also override or inject runtime input without editing the JSON. Generated agents accept flags such as `--input-text`, `--input-url`, and `--input-file`. By default, runtime input flags replace the baked `inputs` array for that run. Use `--input-mode append` to keep baked inputs first, or `--input-mode prepend` to place runtime inputs before the baked inputs. If `agent_schema.properties` is empty, those model-facing runtime input flags are invalid because Cargo AI skips the initial model call in that structural action-only shape.

```bash
./my_agent --input-text "What is 3 + 3?"
```

Top-level inputs may also declare optional `name`. Named inputs stay regular inputs for schema-backed agents, but they also become reusable bindings for child-agent steps and targeted runtime replacement with repeatable `--input-override NAME=VALUE`.
As a rule of thumb, prefer `name` when an input is part of the workflow contract, reusable by child steps, or likely to be operator-overrideable. Leave one-off root-model context unnamed when it does not need that extra identity.
For readability, prefer named input object field order as `name`, then `type`, then the value field. Keep unnamed literal inputs as `type`, then the value field.

```json
{
  "inputs": [
    { "name": "menu_image", "type": "image" },
    { "name": "menu_note", "type": "text", "text": "Use the attached menu image as the source of truth." }
  ]
}
```

```bash
./my_agent \
  --input-override menu_image=./artifacts/menu-spring.png \
  --input-override menu_note="Use the spring menu."
```

You can also declare typed runtime variables for action control and step-local settings. Define them under top-level `runtime_vars`, pass values with repeatable `--run-var name=value`, and reference them in JSON as `runtime.<name>`.

```json
{
  "runtime_vars": {
    "generate_images": { "type": "boolean", "default": false },
    "hero_image_model": { "type": "string", "default": "gpt-image-1.5" }
  }
}
```

```bash
./my_agent \
  --run-var generate_images=true \
  --run-var hero_image_model=gpt-image-1.5
```

Quote `--run-var` values when your shell would otherwise split them, for example `--run-var subject="Quarterly Review"`.

You can also author a structural action-only worker by leaving `agent_schema.properties` empty. In that shape, Cargo AI skips the initial model pass and starts directly at action `logic`, which can read declared `runtime.*` values. Top-level named `inputs` are still allowed there as reusable parent-owned inputs for child forwarding.

```json
{
  "version": "2026-03-03.r1",
  "inputs": [
    { "name": "menu_image", "type": "image" }
  ],
  "runtime_vars": {
    "generate_images": { "type": "boolean", "default": true }
  },
  "agent_schema": {
    "type": "object",
    "properties": {}
  },
  "actions": [
    {
      "name": "generate_launch_assets",
      "logic": { "==": [{ "var": "runtime.generate_images" }, true] },
      "run": [
        {
          "kind": "agent",
          "agent": "./child_renderer",
          "inputs": [
            { "input": "menu_image" },
            { "type": "text", "text": "Create the launch image." }
          ]
        }
      ]
    }
  ]
}
```

```bash
./launch_parent --input-override menu_image=./artifacts/menu-spring.png
```

## Start Simple, Then Expand

Use these snippets to recognize how `inputs`, `agent_schema`, and `actions` grow as the agent becomes more capable.
Click linked labels to open full runnable examples.

### Inputs

Use the input types that fit the job.

[Text input](./templates/shared/examples/text_input_playground.md):

```json
{
  "inputs": [
    { "type": "text", "text": "Summarize the meeting notes." }
  ]
}
```

URL input:

```json
{
  "inputs": [
    { "type": "url", "url": "https://example.com/report" }
  ]
}
```

Image input:

```json
{
  "inputs": [
    { "type": "image", "path": "./invoice.png" }
  ]
}
```

File input:

```json
{
  "inputs": [
    { "type": "file", "path": "./q1-report.pdf" }
  ]
}
```

Named input:

```json
{
  "inputs": [
    { "name": "menu_image", "type": "image" },
    { "name": "menu_note", "type": "text", "text": "Use the attached menu image." }
  ]
}
```

<details>
<summary>Expanded example: multiple inputs with related scoring</summary>

Multiple inputs with related scoring:

```json
{
  "inputs": [
    { "type": "text", "text": "Review this building package and decide how urgently it should be inspected." },
    { "type": "url", "url": "https://example.com/listings/building-123" },
    { "type": "text", "text": "Front facade image for the same building." },
    { "type": "image", "path": "./building-front.png" },
    { "type": "text", "text": "Building specifications and constraints." },
    { "type": "file", "path": "./building-specs.pdf" }
  ],
  "agent_schema": {
    "type": "object",
    "properties": {
      "priority_rank": {
        "type": "integer",
        "minimum": 1,
        "maximum": 5,
        "description": "Inspection priority, where 5 is highest."
      },
      "confidence": {
        "type": "number",
        "exclusiveMinimum": 0,
        "maximum": 1,
        "description": "Confidence in the priority ranking."
      },
      "reason": {
        "type": "string",
        "description": "Short explanation tied to the evidence."
      }
    }
  }
}
```

You can override the baked inputs any time you run the generated agent. By default, runtime input flags replace the configured `inputs` for that execution, and the runtime input order is preserved exactly as you pass it on the command line. Use `--input-mode append` to keep baked inputs first, or `--input-mode prepend` to keep runtime inputs first. When you need to target one declared named input specifically, use repeatable `--input-override NAME=VALUE`.

```bash
./agent_x \
  --input-text "This is the listing page for the building." \
  --input-url "https://example.com/listings/building-456" \
  --input-text "This is the front facade image." \
  --input-image "./building-456-front.png" \
  --input-text "These are the building specifications." \
  --input-file "./building-456-specs.pdf"
```

```bash
./agent_x \
  --input-mode append \
  --input-file "./building-456-specs.pdf"
```

```bash
./agent_x \
  --input-mode prepend \
  --input-text "Read this first." \
  --input-file "./building-456-specs.pdf"
```

</details>

### `agent_schema`

The `agent_schema` is the output contract for the agent. Start simple, then add more structure as the agent becomes more capable.

Minimal output contract:

```json
{
  "agent_schema": {
    "type": "object",
    "properties": {
      "answer": { "type": "integer" }
    }
  }
}
```

Add clearer field meaning with descriptions:

```json
{
  "agent_schema": {
    "type": "object",
    "properties": {
      "summary": {
        "type": "string",
        "description": "One-sentence summary for the operator."
      },
      "needs_follow_up": {
        "type": "boolean",
        "description": "Whether a human should review the result."
      }
    }
  }
}
```

`agent_schema` can include any number of top-level `string`, `integer`, `number`, and `boolean` fields, plus optional `description`, string `enum`, and numeric bounds where supported.

<details>
<summary>Expanded example: richer constraints and exact output choices</summary>

Then expand into richer constraints and exact output choices:

```json
{
  "agent_schema": {
    "type": "object",
    "properties": {
      "priority_rank": {
        "type": "integer",
        "minimum": 1,
        "maximum": 5,
        "description": "Inspection priority, where 5 is highest."
      },
      "confidence": {
        "type": "number",
        "exclusiveMinimum": 0,
        "maximum": 1,
        "description": "Confidence in the priority ranking."
      },
      "status": {
        "type": "string",
        "enum": ["clear", "review", "urgent"],
        "description": "Final triage status."
      },
      "reason": {
        "type": "string",
        "description": "Short explanation tied to the evidence."
      }
    }
  }
}
```

</details>

### `actions`

`actions` define what the agent is allowed to do after it produces the top-level structured output.
Action `logic` uses [JSON Logic](https://jsonlogic.com/).
Within an action, run steps execute in order after the action's JSON Logic condition evaluates true. That logic can read both top-level model output fields and declared `runtime.*` values.
By default, a failed step stops the rest of that action's `run` list unless you set `failure_mode: "continue"`, but later eligible top-level actions still run and Cargo AI aggregates top-level failures at the end. If a step is truly fatal for the whole invocation, use `failure_mode: "abort"` to stop scheduling new work, let already-running work settle, and fail the run with an explicit abort summary.

Start with one simple local action:

```json
{
  "actions": [
    {
      "name": "save_note",
      "logic": {
        "and": [
          { "==": [ { "var": "needs_follow_up" }, true ] },
          { ">": [ { "var": "confidence" }, 0.6 ] }
        ]
      },
      "run": [
        {
          "kind": "exec",
          "program": "./save_note",
          "args": [{ "var": "summary" }]
        }
      ]
    }
  ]
}
```

<details>
<summary>Expanded example: multiple action types</summary>

Then expand into multiple action types:

```json
{
  "actions": [
    {
      "name": "save_locally",
      "logic": {
        "and": [
          { "==": [ { "var": "status" }, "review" ] },
          { ">=": [ { "var": "priority_rank" }, 3 ] }
        ]
      },
      "run": [
        {
          "kind": "exec",
          "program": "./save_report.sh",
          "args": [{ "var": "reason" }]
        }
      ]
    },
    {
      "name": "email_operator",
      "logic": {
        "or": [
          { "==": [ { "var": "status" }, "urgent" ] },
          {
            "and": [
              { ">=": [ { "var": "priority_rank" }, 4 ] },
              { ">": [ { "var": "confidence" }, 0.85 ] }
            ]
          }
        ]
      },
      "run": [
        {
          "kind": "email_me",
          "subject": "Urgent building review",
          "text": ["Reason: ", { "var": "reason" }]
        }
      ]
    },
    {
      "name": "handoff_to_child",
      "logic": {
        "and": [
          { "==": [ { "var": "status" }, "review" ] },
          { "<": [ { "var": "confidence" }, 0.75 ] }
        ]
      },
      "run": [
        {
          "kind": "agent",
          "agent": "./child_reporter",
          "inputs": [
            {
              "type": "text",
              "text": ["Follow up on this building package: ", { "var": "reason" }]
            }
          ]
        }
      ]
    }
  ]
}
```

</details>

You can keep actions simple or mix local executables, email alerts, child-agent handoffs, and generated image artifacts in the same agent definition. The next section shows how to sequence multiple run steps and control them with `when`.

Top-level actions run `sequential`ly by default. If you want matching top-level actions to overlap, add:

```json
{
  "action_execution": "parallel"
}
```

That only changes scheduling across top-level actions. Each individual action still keeps its own `run` list in order, and a hard failure in one top-level action no longer prevents later eligible top-level actions from running. Cargo AI aggregates those top-level hard failures after all eligible actions finish.

Cargo AI prints one root `using:` line near run start that shows the effective `profile`, `auth`, `server`, and `model` for that invocation. It only adds `url=...` when the effective URL is custom or materially different from the standard transport. Cargo AI also prints one run-level mode header before actions start. When output is redirected, piped, or running in simpler terminals, it prefixes parent-visible action output with deterministic labels such as `[Action 1: first_action]`, long-running steps emit a step-start liveness line such as `step 2/2 generate_image started; waiting for provider response...`, and terminal lane summaries plus the root run footer include wall-clock durations such as `completed in 31s.` and `✅ Run complete in 32s.`. When attached directly to an interactive terminal, it switches to a compact live dashboard that groups each action by label, running or terminal status with elapsed time, terminal step marker/current step, and the last high-level lifecycle message only. Child-agent steps stay minimal in the parent view with start/completion or exit summaries instead of recursively inlining child detail.

Use `--render-mode auto|live|append-only` to control that behavior explicitly:
- `auto` preserves the current terminal-sensitive default
- `append-only` forces incremental labeled output even in an interactive terminal
- `live` forces the dashboard when supported and otherwise falls back to append-only with a short notice

If you need a safety/testing pass, invoke a parallel-capable agent with `--action-execution sequential`. That runtime override forces the whole invocation tree down to sequential scheduling for that run, including child-agent handoffs.

### `run`

`run` is the ordered step list inside an action.

Start with one simple step:

```json
{
  "run": [
    {
      "kind": "exec",
      "program": "./save_report.sh",
      "args": [{ "var": "reason" }]
    }
  ]
}
```

<details>
<summary>Expanded example: multi-step workflow</summary>

Then expand into a multi-step workflow:

```json
{
  "run": [
    {
      "kind": "exec",
      "program": "./save_report.sh",
      "args": [{ "var": "reason" }],
      "output_variable": "report_path",
      "status_variable": "save_status",
      "error_variable": "save_error",
      "failure_mode": "continue"
    },
    {
      "kind": "email_me",
      "when": {
        "and": [
          { "==": [ { "var": "save_status" }, "succeeded" ] },
          { ">=": [ { "var": "priority_rank" }, 4 ] }
        ]
      },
      "subject": "Building report saved",
      "text": ["Saved report to ", { "var": "report_path" }]
    },
    {
      "kind": "agent",
      "when": { "==": [ { "var": "save_status" }, "failed" ] },
      "agent": "./child_reporter",
      "inputs": [
        {
          "type": "text",
          "text": ["Saving failed for this building review: ", { "var": "save_error" }]
        },
        {
          "type": "text",
          "text": ["Original reason: ", { "var": "reason" }]
        }
      ]
    }
  ]
}
```

</details>

Use `run` to sequence multiple side effects in order. `exec` steps can capture output, status, or errors for later steps, `generate_image` can write a single local image artifact, and `when` lets later steps react to success or failure without leaving the agent definition.

`generate_image.model` is optional. If omitted, Cargo AI falls back to the effective invocation model resolved from the current profile and any `--model` CLI override. If neither the step nor the invocation provides a model, the run fails clearly instead of guessing. When the image step should use a different model from the main invocation, set `generate_image.model` explicitly as either a literal string or a single variable reference. Prefer a runtime-backed string such as `{ "var": "runtime.hero_image_model" }` when the operator should choose the image model at invocation time. Top-level string schema fields may also drive `generate_image.model`, but captured step variables may not.

`generate_image` and child `agent` steps also accept an optional step-level `profile`. Use it when one step should resolve its provider/model/url/token context differently from the parent invocation. For `generate_image`, explicit `model` still wins, then the step-profile model, then the parent invocation model. That means a parent agent may stay on OpenAI while one `generate_image` step switches to an Ollama profile. For child `agent` steps, the resolved profile is forwarded to the child as `--profile <name>`.

Cargo AI always prints one root `using:` line near run start. In append-only output, it also prints another action-prefixed `using:` line when a provider-backed or child-agent step changes the effective `profile`, `auth`, `server`, or `model`. Interactive live mode keeps the parent dashboard at the orchestration level and does not surface child or step-level `using:` lines there.

For the default OpenAI account transport, use a tool-capable mainline model such as `gpt-5.2`. For a direct OpenAI API token and URL, prefer GPT Image models such as `gpt-image-1.5` or `gpt-image-1-mini`. Official OpenAI docs list `gpt-image-1.5` as the latest GPT Image model, and the image-generation guide lists `gpt-image-1.5`, `gpt-image-1`, and `gpt-image-1-mini` for direct image generation. Verified: 2026-03-28. For Ollama's experimental OpenAI-compatible `/v1/images/generations` endpoint, use an Ollama image model such as `x/flux2-klein:4b` on a step-level Ollama profile. The current Cargo AI compatibility slice uses Ollama's documented `b64_json` response path, so Ollama-backed `generate_image` steps currently require a `.png` output path.

```json
{
  "kind": "generate_image",
  "profile": { "var": "runtime.image_profile" },
  "model": { "var": "runtime.hero_image_model" },
  "prompt": ["Create a product render for ", { "var": "reason" }],
  "path": "./artifacts/product_render.png"
}
```

You can also target individual run steps to specific runtime platforms:

```json
{ "kind": "exec", "program": "./save_report.sh", "platform": "macos", "args": [{ "var": "reason" }] }
```

Or target multiple platforms with an array:

```json
{ "kind": "exec", "program": "./save_report.sh", "platform": ["macos", "linux", "windows"], "args": [{ "var": "reason" }] }
```

### Child agents

Use child agents when one agent needs to hand work to another agent.

- Point to a child agent that lives next to the parent file, such as `./child_reporter`.
- By default, an agent can call child agents up to `5` levels deep. Override that with `--max-agent-depth`.
- By default, the parent plus any child agents share a total runtime budget of `600` seconds. Override that with `--max-runtime-in-sec`.
- A parent can pass inputs to a child and record whether the child succeeded or failed.
- A parent can also reuse one declared named top-level input explicitly inside child `inputs` with `{ "input": "<name>" }`.
- Child `agent` steps may set `run_vars` to pass child runtime vars the same way the CLI uses repeatable `--run-var NAME=VALUE`.
- Child `agent` steps may set `input_overrides` to target the child's declared named inputs directly.
- Child `agent` steps may still provide anonymous child `inputs`.
- Child `agent` steps may set `input_mode` to `replace`, `append`, or `prepend` when they also provide child `inputs`.
- Named child-input reuse is explicit only. Cargo AI does not automatically inherit every named parent input into the child.
- If a middle agent wants to pass the same named input to its own child, it should declare the same named top-level input locally first.
- `run_vars`, `input_overrides`, `inputs`, and `input_mode` mirror the CLI mental model:
  - `run_vars` is the child-step equivalent of `--run-var NAME=VALUE`
  - `input_overrides` is the child-step equivalent of `--input-override NAME=VALUE`
  - `inputs` is the child-step equivalent of anonymous runtime `--input-*`
  - `input_mode` applies only to child `inputs`, not to `input_overrides`
- Prefer `input_overrides` when targeting declared named child inputs. Use child `inputs` for extra anonymous context.
- If the target is another Cargo AI agent, prefer a native `kind: "agent"` step instead of a Python or shell wrapper that only launches the child.
- Use wrapper programs only when the task truly needs extra non-Cargo-AI behavior around that child call.
- A parent cannot automatically pull the child's structured return fields back into its own output.

Assume the parent definition also declares `{ "name": "menu_image", "type": "image" }` at top level.

Example:

```json
{
  "kind": "agent",
  "agent": "./child_reporter",
  "profile": { "var": "runtime.child_profile" },
  "run_vars": {
    "year": { "var": "runtime.year" },
    "month": "08",
    "generate_images": true
  },
  "input_overrides": {
    "menu_image": { "input": "menu_image" },
    "review_reason": { "var": "reason" }
  },
  "input_mode": "append",
  "status_variable": "child_status",
  "error_variable": "child_error",
  "inputs": [
    {
      "type": "text",
      "text": "Follow up on the latest review details."
    }
  ]
}
```

That child step behaves like a structured CLI invocation:

- `run_vars.year` is equivalent to `--run-var year=...`
- `run_vars.month` is equivalent to `--run-var month=08`
- `run_vars.generate_images` is equivalent to `--run-var generate_images=true`
- `input_overrides.menu_image` is equivalent to `--input-override menu_image=...`
- `input_overrides.review_reason` is equivalent to `--input-override review_reason=...`
- child `inputs` stays the anonymous extra-input list
- child `input_mode` still controls only that anonymous `inputs` list

Use these child-step value shapes:

- `run_vars.<name>`: string, number, boolean, or `{ "var": "..." }`
- `input_overrides.<name>`: string, `{ "var": "..." }`, or `{ "input": "<name>" }`

For schema-backed agents, `--input-override` and anonymous runtime inputs operate at different layers. This is valid:

```bash
./menu_agent \
  --input-override menu_image=./artifacts/menu-spring.png \
  --input-text "Ignore baked inputs and use this prompt"
```

In that case, the root model input list is replaced by the runtime text, but child steps that use `{ "input": "menu_image" }` still receive the named override.

## Build In Any Editor

You can build a `cargo-ai` agent in any editor you want. If you want the fastest execution loop while editing, run the JSON directly:

```bash
cargo ai run --config ./my_agent.json --profile openai-account
```

If you want to check whether the definition is valid before exporting a binary, run:

```bash
cargo ai hatch my_agent --config ./my_agent.json --check
```

If your config file already matches the agent name, the shorthand works too:

```bash
cargo ai hatch my_agent.json --check
```

When the file checks cleanly, use the Codex workflow below for the fastest iteration loop.

## Best First Workflow in Codex

If you want the fastest authoring loop, start in a new folder and let Codex build the agent definition with you.

```bash
mkdir my-agent
cd my-agent
cargo ai add guidance --style codex
codex
```

This creates `AGENTS.md` plus helper files under `.cargo-ai/guidance/` so Codex knows the Cargo AI contract.

Then tell Codex: `I want to build a Cargo AI agent.` Describe what the agent should do, what inputs it should accept, what structured output it should return, and any follow-up actions you want.

Ask Codex to:

- build the JSON definition
- run `cargo ai hatch my_agent --config ./my_agent.json --check`
- update the JSON until the check passes

Then review the generated JSON yourself to make sure it matches your intent.

Cargo AI works best when the definition stays small, understandable, and easy to verify as you iterate.

## Account-Backed Flows

After registration, you can use Cargo AI as more than a local hatching tool:

- store and retrieve agent definitions through your account
- hatch from your own hosted definitions
- hatch public definitions from another owner's handle
- use account-aware email workflows

Examples:

```bash
# Hatch your own hosted definition
cargo ai account hatch weather_test

# Validate scaffold and compile path without exporting a binary
cargo ai account hatch weather_test --check

# Hatch a public definition from another handle
cargo ai account agents hatch weather_test --owner-handle alice
```

## Where To Go Next

When you want deeper details, use these files:

- Versioning and releases:
  - [VERSIONING.md](./VERSIONING.md)
  - [releases/0.2.0.md](./releases/0.2.0.md)
- Examples:
  - [adder_test.json](./adder_test.json)
  - [weather_test.json](./weather_test.json)
- JSON/schema reference:
  - [templates/shared/docs/schema-quick-reference.md](./templates/shared/docs/schema-quick-reference.md)
  - [templates/guidance/agent-definition-contract.md](./templates/guidance/agent-definition-contract.md)
- Actions and authoring patterns:
  - [templates/guidance/action-rules.md](./templates/guidance/action-rules.md)
  - [templates/guidance/authoring-patterns.md](./templates/guidance/authoring-patterns.md)
  - [templates/guidance/examples/README.md](./templates/guidance/examples/README.md)
- Hatch/check workflow:
  - [templates/shared/docs/hatch-check-loop.md](./templates/shared/docs/hatch-check-loop.md)
- Troubleshooting:
  - [templates/guidance/troubleshooting.md](./templates/guidance/troubleshooting.md)

## Notes

- `cargo ai hatch --check` validates scaffold and compile behavior with `cargo check` without exporting a binary.
- Generated binaries use your configured/default profile unless you override runtime flags.
- Standalone recipients do not need Cargo AI installed if they run the binary with explicit runtime flags such as `--server`, `--model`, optional `--url`, optional `--token`, and optional `--render-mode`.
- `--profile <name>` is strict for generated binaries: if the named profile is missing, the run fails closed instead of falling back to another profile or to profileless auth.
- For the standalone OpenAI account path, run the generated binary with `--server openai --model <model>` and no `--token`; if a local Codex session is available, the binary reuses it automatically.
- On machines without Cargo AI installed/configured, `./my_agent version` treats local sync comparison as not checked and points users to `./my_agent inspect` for embedded provenance.
- Scheduling is not built into Cargo AI today. To run an agent on a schedule, use your operating system scheduler such as `cron` on macOS/Linux or Task Scheduler on Windows. We know scheduling matters and expect this area to expand over time.
- Cargo AI recommends manual upgrade via:

```bash
cargo install cargo-ai --locked
```

## License

MIT. See [LICENSE](./LICENSE).
