# cargo-ai™

[![Audit Status](https://github.com/analyzer1/cargo-ai/actions/workflows/security-audit.yml/badge.svg)](https://github.com/analyzer1/cargo-ai/actions/workflows/security-audit.yml)
[![Multi-OS CI](https://github.com/analyzer1/cargo-ai/actions/workflows/multi-os-ci.yml/badge.svg)](https://github.com/analyzer1/cargo-ai/actions/workflows/multi-os-ci.yml)
[![Status: Stable – Ongoing Development](https://img.shields.io/badge/Status-Stable_–_Ongoing_Development-blue)](https://github.com/analyzer1/cargo-ai)

Declarative AI agents. Native executables. Shareable in minutes.

An open-source toolkit for building agents that are clear, portable, and fully auditable. Define exactly what the agent does in a concise JSON file, refine it quickly with AI tools, including Codex, hatch it into a native executable, and keep complete control over the result.

```bash
cargo ai hatch agent_x
```

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

### 3. Hatch a sample agent

```bash
cargo ai hatch adder_test
./adder_test
```

On Windows, run `adder_test.exe` or just `adder_test`.

### 4. Register an account

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

Cargo AI keeps the authoring model intentionally small:

1. `inputs`
   Ordered model-facing input such as `text`, `url`, or `image`.
2. `agent_schema`
   The typed response you expect back.
3. `actions`
   What to do after the response is validated.

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

That JSON becomes a compiled local executable through:

```bash
cargo ai hatch my_agent --config ./my_agent.json
./my_agent
```

For Windows users, run `my_agent.exe` or just `my_agent`.

You can also override or inject runtime input without editing the JSON. Generated agents accept flags such as `--input-text`, `--input-url`, and `--input-file`.

```bash
./my_agent --input-text "What is 3 + 3?"
```

## Start Simple, Then Expand

The example above is intentionally simple. `cargo-ai` can work with richer inputs, logic, and actions while keeping the agent definition concise and auditable.

### Inputs

Use the input types that fit the job.

Text input:

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

You can override the baked inputs any time you run the generated agent. Runtime input flags replace the configured `inputs` for that execution, and the order is preserved exactly as you pass it on the command line.

```bash
./agent_x \
  --input-text "This is the listing page for the building." \
  --input-url "https://example.com/listings/building-456" \
  --input-text "This is the front facade image." \
  --input-image "./building-456-front.png" \
  --input-text "These are the building specifications." \
  --input-file "./building-456-specs.pdf"
```

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

`agent_schema` can include any number of top-level `string`, `integer`, `number`, and `boolean` fields, plus optional `description`, string `enum`, and numeric bounds where supported.

### `actions`

`actions` define what the agent is allowed to do after it produces the top-level structured output.
Action `logic` uses [JSON Logic](https://jsonlogic.com/).
Within an action, run steps execute in order after the action's JSON Logic condition evaluates true.

Start with one simple local action:

```json
{
  "actions": [
    {
      "name": "save_note",
      "logic": { "==": [ { "var": "needs_follow_up" }, true ] },
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

Then expand into multiple action types:

```json
{
  "actions": [
    {
      "name": "save_locally",
      "logic": { "==": [ { "var": "status" }, "review" ] },
      "run": [
        {
          "kind": "exec",
          "program": "./save_report",
          "args": [{ "var": "reason" }]
        }
      ]
    },
    {
      "name": "email_operator",
      "logic": { "==": [ { "var": "status" }, "urgent" ] },
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
      "logic": { "==": [ { "var": "status" }, "review" ] },
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

You can keep actions simple or mix local executables, email alerts, and child-agent handoffs in the same agent definition. The next section shows how to sequence multiple run steps and control them with `when`.

## Best First Workflow in Codex

If you are building with Codex, this is the simplest path:

1. Add local authoring guidance:

```bash
cargo ai add guidance --style codex
```

2. Start from a sample config:
   - [adder_test.json](./adder_test.json)
   - [weather_test.json](./weather_test.json)

3. Ask Codex to modify the JSON, not to build a whole framework.
4. Re-run the loop:

```bash
cargo ai hatch my_agent --config ./my_agent.json --check
```

5. Hatch the real binary when the definition is ready:

```bash
cargo ai hatch my_agent --config ./my_agent.json
```

The point of Cargo AI is not to make you manage more code. It is to let you keep the agent definition small, understandable, and easy to iterate.

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

## Reference

Use the top-level README for orientation. Use these files for the deeper details:

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
- Cargo AI recommends manual upgrade via:

```bash
cargo install cargo-ai --locked
```

## License

MIT. See [LICENSE](./LICENSE).
