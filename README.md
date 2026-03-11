# cargo-ai™

[![Audit Status](https://github.com/analyzer1/cargo-ai/actions/workflows/security-audit.yml/badge.svg)](https://github.com/analyzer1/cargo-ai/actions/workflows/security-audit.yml)
[![Multi-OS CI](https://github.com/analyzer1/cargo-ai/actions/workflows/multi-os-ci.yml/badge.svg)](https://github.com/analyzer1/cargo-ai/actions/workflows/multi-os-ci.yml)
[![Status: Stable – Ongoing Development](https://img.shields.io/badge/Status-Stable_–_Ongoing_Development-blue)](https://github.com/analyzer1/cargo-ai)

Declarative AI agents. Native executables. Shareable in minutes.

An open-source toolkit for building agents that are clear, portable, and fully auditable. Define exactly what the agent does in a concise JSON file, refine it quickly with AI tools, including Codex, hatch it into a native executable, and keep complete control over the result.

```bash
cargo ai hatch weather_test --config weather_test.json
```

## Why Cargo AI

- **Declarative by Design**: define exactly what the agent does, what actions it can take, and keep the behavior easy to inspect.
- **You Own the Output**: hatch a local executable and generated code that you can keep, modify, and run wherever you want.
- **Open Source and Fully Auditable**: inspect the generated code, understand what ships, and keep control of the runtime.
- **Choose Your Own AI**: use OpenAI models today or open-source models through Ollama, with room for more providers over time.
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

Option A: recommended if you already use OpenAI + Codex

This is the easier path today if you already use OpenAI tools like Codex. `cargo-ai` uses your Codex login state so you do not need to paste a separate API key into Cargo AI.

```bash
codex login

cargo ai profile add openai-account \
  --server openai \
  --model gpt-4o \
  --auth openai_account \
  --default

cargo ai auth login openai --profile openai-account --set-default
```

If you do not already have Codex installed, get it here:
[Codex CLI setup](https://developers.openai.com/codex/cli)

Option B: direct provider control

Use this path if you want an explicit model profile with direct provider credentials and no Codex dependency.

```bash
cargo ai profile add openai \
  --server openai \
  --model gpt-4o \
  --auth api_key \
  --default

cargo ai profile set openai --token sk-*** --auth api_key
```

### 3. Hatch a sample agent

```bash
cargo ai hatch adder_test
./adder_test
```

On Windows, run `adder_test.exe` or just `adder_test`.

### 4. Register an account

If you want `cargo-ai.org` features such as shareable definitions, your public handle, and email-backed workflows, create an account early:

```bash
cargo ai account register you@example.com
cargo ai account confirm <code-from-email>
cargo ai account handle --set yourname
```

Once registered, you can pull definitions from your account repository and hatch them locally:

```bash
cargo ai account hatch weather_test
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
  "version": 1,
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

## What You Can Build Today

- Local agents hatched from JSON files
- Account-hosted agents you can hatch by name
- Typed outputs with flat top-level scalar fields
- Conditional follow-up behavior through JSON Logic
- Action steps that run:
  - local commands via `exec`
  - account-backed email via `email_me`
  - child agents via `agent`
- URL inputs fetched by Cargo AI's own HTTP client with a practical compatibility target comparable to `curl` for ordinary static or server-rendered content

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
