# cargo-ai

[![Audit Status](https://github.com/analyzer1/cargo-ai/actions/workflows/security-audit.yml/badge.svg)](https://github.com/analyzer1/cargo-ai/actions/workflows/security-audit.yml)
[![Multi-OS CI](https://github.com/analyzer1/cargo-ai/actions/workflows/multi-os-ci.yml/badge.svg)](https://github.com/analyzer1/cargo-ai/actions/workflows/multi-os-ci.yml)
[![Status: Stable – Ongoing Development](https://img.shields.io/badge/Status-Stable_–_Ongoing_Development-blue)](https://github.com/analyzer1/cargo-ai)

Build AI agents in plain JSON, hatch them into native binaries, and iterate in Codex without hand-writing a Rust CLI.

`cargo-ai` is for people who want the clarity of a simple config file with the reliability of a compiled tool:
- define the agent in JSON
- hatch it into a local executable
- run it with your preferred model profile
- add local commands, account-backed email actions, or other agents as follow-up steps

It is especially strong when you want to build and refine agents inside Codex. The JSON is readable, diffable, and easy to keep improving without dropping into a full application codebase.

## Why Cargo AI

- **Readable by design**: the agent definition is a small JSON file, not a framework project.
- **No hand-built CLI required**: Cargo AI generates the executable and the typed runtime scaffolding for you.
- **Good for non-technical and semi-technical builders**: you can understand the shape of the agent visually and grow it step by step.
- **Works locally first**: hatch and run from your machine before worrying about hosted definitions.
- **Account flows are built in**: register once to unlock hosted agent definitions, handles, and account-backed email workflows.

## Quick Start

### 1. Install

```bash
cargo install cargo-ai --locked
cargo ai --help
```

Check for updates later with:

```bash
cargo ai version --check
```

### 2. Add a default model profile

API-key path:

```bash
cargo ai profile add openai \
  --server openai \
  --model gpt-4o \
  --auth api_key \
  --default

cargo ai profile set openai --token sk-*** --auth api_key
```

OpenAI account-login path:

```bash
cargo ai profile add openai-account \
  --server openai \
  --model gpt-4o \
  --auth openai_account \
  --default

cargo ai auth login openai --profile openai-account --set-default
```

### 3. Hatch a sample agent

```bash
cargo ai hatch adder_test
./adder_test
```

On Windows, run `adder_test.exe` or just `adder_test`.

### 4. Register an account

If you want hosted definitions, handles, and email-backed account workflows, create an account early:

```bash
cargo ai account register you@example.com
cargo ai account confirm <code-from-email>
cargo ai account handle --set yourname
```

Once registered, you can hatch account-hosted agents directly:

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
