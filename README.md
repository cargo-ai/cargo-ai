# cargo-ai

[![Audit Status](https://github.com/analyzer1/cargo-ai/actions/workflows/security-audit.yml/badge.svg)](https://github.com/analyzer1/cargo-ai/actions/workflows/security-audit.yml)
[![Multi-OS CI](https://github.com/analyzer1/cargo-ai/actions/workflows/multi-os-ci.yml/badge.svg)](https://github.com/analyzer1/cargo-ai/actions/workflows/multi-os-ci.yml)
[![Status: Stable – Ongoing Development](https://img.shields.io/badge/Status-Stable_–_Ongoing_Development-blue)](https://github.com/analyzer1/cargo-ai)
> **Early stable (< v0.1):** reliable base ready for use, with evolving APIs and config patterns.

## 🌐 Overview
`cargo-ai` is a lightweight, Rust-based framework for building **no-code AI agents** using clean, declarative, JSON configs. Agents compile into fast, secure binaries—perfect for local machines, servers, and with broader embedded device support planned.

Supports both **OpenAI‑API‑compatible servers** and **Ollama**.

*Lightweight AI agents. Built in Rust. Declared in JSON.*

## ✨ Features

- **Declarative, No-Code Agents** – Define agent logic in JSON  
- **Portable JSON Configs** – Share agent definitions as JSON; others can "hatch" and run them on their own systems
- **Full CLI Integration** – Conformed agent outputs can run an arbitrary command-line program
- **Rust-Powered** – Safe, fast, and portable across environments  
- **Fully Local & Secure** – All logic executes client-side (no phoning home)  
- **LLM Connection Profiles** – Store reusable settings for servers, models, auth modes, and timeouts so you don't re-enter them each run
- **Repository Integration** – Download JSON configurations directly from Cargo-AI and hatch agents without needing local files
- **Cross‑Platform Support** – Runs on any Linux, macOS, or Windows device

## 🧭 Internal Layout (CLI)

- `src/main.rs` keeps runtime dispatch thin and routes work into `src/commands/*`.
- `src/args.rs` is the parser root and composes command parsers from `src/args/*`.
- `src/commands/*` owns command behavior (`preflight`, `hatch`, `profile`, `shipyard`, `account`).
- `src/commands/account/*` and `src/args/account/*` keep account subcommand paths explicit and testable.
- `templates/build_support.rs` is the shared build-time hardening/codegen logic used by both build scripts.

## 🚀 Upcoming Features

- **User Repositories (Public & Private)** – Publish agents to your own hosted repository and share them publicly or privately with collaborators.
- **Email Actions** – Enable agents to send automated emails as action outputs, expanding beyond command-line execution.

## 🔮 Future Features
- **Microcontroller Support** – Planned support for ultra‑lightweight environments, expanding beyond standard servers to microcontroller‑class devices

## 📦 Installation

### Base Install

1. **Install Rust & Cargo**  
   Follow the official guide:  
   [Install Rust & Cargo](https://doc.rust-lang.org/cargo/getting-started/installation.html)

2. **Install cargo-ai**  
   Once Cargo is available, install `cargo-ai`:  
   ```bash
   cargo install cargo-ai
   ```

    Verify installation:  
    ```bash
    cargo ai --help
    ```

## ⚡ Quick Start

### Configure an LLM Profile (Recommended)

Before hatching or running agents, it is recommended to set up a default LLM connection profile.  
This allows `cargo-ai` to run agents without requiring server, model, or token flags each time.

#### Add a Default OpenAI Profile

Example (using OpenAI GPT 4o):

```bash
cargo ai profile add openai \
    --server openai \
    --model gpt-4o \
    --auth api_key \
    --default
```

Set token-based credentials:

```bash
cargo ai profile set openai --token sk-*** --auth api_key
```

Or use OpenAI account login instead of API key:

```bash
cargo ai profile add openai-account --server openai --model gpt-5.2 --auth openai_account --default
cargo ai auth login openai --profile openai-account --set-default
```

`cargo ai auth login openai` follows OpenAI/Codex browser sign-in semantics and reads session tokens from Codex local auth storage (`$CODEX_HOME/auth.json`, or `~/.codex/auth.json` by default) without importing duplicate OpenAI account tokens into Cargo AI secret stores.

Cargo-AI supports Ollama and OpenAI‑compatible transformer servers. To change the default URL, use:
```bash
--url <custom_llm_endpoint>
```

#### Credential Storage (Phase 1)

- `config.toml` is metadata-only and no longer persists profile/account secret values.
- `config.toml` also carries non-secret local Cargo AI metadata in `[cargo_ai_metadata]`, including the installed Cargo AI version, template schema version, build target, install ID, and binary SHA-256.
- Default secret-store mode is `file` for new installs.
- `file` mode stores secrets in `credentials.toml` at:
  - `$CARGO_HOME/.cargo-ai/credentials.toml`, or
  - `~/.cargo/.cargo-ai/credentials.toml`.
- `keychain` mode stores secrets in OS keychain backends through Rust `keyring` (3.x stable).
- Manage mode with:
  - `cargo ai credentials store status`
  - `cargo ai credentials store set <file|keychain>`
  - `cargo ai credentials store set <file|keychain> --migrate --yes`
- Manage OpenAI auth session with:
  - `cargo ai auth login openai [--profile <name>] [--set-default]`
  - `cargo ai auth status [--json]`
  - `cargo ai auth logout [--global] [--yes]`
- `cargo ai auth logout` is local-only by default (Cargo AI stops using OpenAI account auth, Codex remains signed in).
- Use `cargo ai auth logout --global` to also run `codex logout`.
- Manage profile metadata/auth/token material with:
  - `cargo ai profile add <name> --server <server> --model <model> [--auth <none|api_key|openai_account>] [--default]`
  - `cargo ai profile set <name> [--server <server>] [--model <model>] [--auth <none|api_key|openai_account>] [--url <URL> | --clear-url] [--description <TEXT> | --clear-description] [--token <TOKEN> | --stdin | --env <ENV_VAR> | --clear-token] [--default]`
  - `cargo ai profile list`
  - `cargo ai profile show <name>`
  - `cargo ai profile remove <name>`
- Use `--migrate --dry-run` to preview migration without writes.
- Fresh installs can switch to `keychain` before any secrets are created (metadata-only switch).
- Legacy secrets found in `config.toml` are migrated once at startup into the active secret-store path.
- Generated agents use the same secret-store mode behavior, so default-profile token usage remains compatible.

### Create a Sample Agent

### Add Codex Guidance

To add a local `AGENTS.md` file for Codex in the current directory:

```bash
cargo ai add guidance --style codex
```

This writes `AGENTS.md` only. It does not create `.cargo-ai/project.toml`, example JSON files, or other scaffold assets.
The generated guidance is tuned for authoring Cargo AI JSON definitions directly and validating them with the hatch/check loop.

1. **Hatch a Sample Agent**  

   By default, if you don’t provide a config file, `cargo-ai` will hatch a sample “Hello World” style agent (`adder_test`) that simply adds 2 + 2.

   Default example:  
   ```bash
   cargo ai hatch adder_test
   ```

   To hatch your own custom agent from a JSON file, see the section **Create Your Own Weather Agent with JSON** below.

### Run the Sample Agent

2. **Run the compiled agent** using your default profile:

   ```bash
   ./adder_test
   ```

   Example output:

   ```
   Using default profile 'openai'
   Running 'is_4': echo ["Value return is equal to 4."]
   Value return is equal to 4.
   Command completed successfully.
   ```

   You can override any part of the default profile at runtime using command‑line flags.  
   For a full listing of options, run:

   ```bash
   ./adder_test --help
   ```
   > **Note for Windows users:**  
   > On Windows, the agent binary will be created with a `.exe` extension (e.g., `adder_test.exe`).  
   > You can run it by simply typing `adder_test` in PowerShell or Command Prompt (the `.exe` is implied).  
   > On macOS and Linux, run the binary from the current directory using `./adder_test`.

### 🧠 Understanding the Sample Agent

  To better understand how agents are created, you can hatch an agent using the generic form of the command:

  ```bash
  cargo ai hatch <AgentName> --config <path_to_json>
  ```
 
  This allows you to leverage either the Cargo‑AI repo or a local `.json` file.  
  For example, using the same `adder_test.json` stored locally:

  ```bash
  cargo ai hatch adder_test2 --config ~/Developer/cargo-ai/adder_test.json
  ```

  This will create a new agent project named `adder_test2` using the contents of your local JSON file.

  Hatch builds now seed from a Cargo AI-owned warmed template cache under `~/.cargo/.cargo-ai/templates/<cargo-ai-binary-sha256>/<rustc-version>/<target-triple>/`.
  The first hatch for a new cache key builds that internal template once; later hatches for the same key reuse it.
  After the active template bucket is confirmed good, Cargo AI prunes stale older Cargo AI hash and `rustc` parent cache directories while preserving sibling target-triple buckets under the active parent.

  To build for an explicit Rust target triple, pass `--target` through to Cargo:

  ```bash
  cargo ai hatch adder_test2 --config ~/Developer/cargo-ai/adder_test.json --target aarch64-apple-darwin
  ```

  To export the built binary to a specific directory while keeping the binary name based on `<name>`:

  ```bash
  cargo ai hatch adder_test2 --config ~/Developer/cargo-ai/adder_test.json --output-dir ./dist
  ```

  Cargo AI does not install Rust targets automatically. Generated agents now use Rustls-backed `reqwest` in the default template to avoid the prior common OpenSSL cross-compilation blocker, but if the requested target is missing or the linker/SDK/sysroot toolchain for that target is incomplete, Cargo AI still surfaces the underlying Cargo/Rust error directly.

  By default, Cargo AI still deletes the internal workspace after build/check. To keep it for inspection:

  ```bash
  cargo ai hatch adder_test2 --config ~/Developer/cargo-ai/adder_test.json --keep-project
  ```

  This preserves the internal project under `~/.cargo/.cargo-ai/agents/<name>/`.
  If a kept workspace already exists, re-run with `--force` to replace it.

### Hatch from Account Agents

When you are signed in, you can hatch an agent directly from account-hosted definitions:

```bash
# Hatch your own account agent (path defaults to "/")
cargo ai account agents hatch weather_test

# Shortcut alias for the same account-hosted hatch flow
cargo ai account hatch weather_test

# Validate scaffold and compile path only (no binary export)
cargo ai account hatch weather_test --check

# Use a different remote account agent while keeping a local output name
cargo ai account hatch weather_test_local --agent weather_test_remote

# Export the built binary to a specific directory
cargo ai account hatch weather_test --output-dir ./dist

# Overwrite existing local output binary
cargo ai account agents hatch weather_test --force

# Preserve the internal project workspace for inspection
cargo ai account agents hatch weather_test --keep-project

# Build an account-hosted agent for an explicit Rust target triple
cargo ai account agents hatch weather_test --target aarch64-apple-darwin
```

To hatch a public agent from another owner:

```bash
cargo ai account agents hatch weather_test --owner-handle alice
```

To select a non-root definition path:

```bash
cargo ai account agents hatch weather_test --definition-path /team/ops
```

  To understand what is happening behind the scenes, we can look at the internal structure of the sample agent JSON file, [`adder_test.json`](./adder_test.json). 

  ### 1. Inputs and Guaranteed Typed Response

  Each agent defines an ordered set of **inputs** together with a strongly‑typed **response schema**.  
  
  The schema is compiled into Rust types, guaranteeing that the agent will always receive data in the expected shape.

  ```json
  {
    "inputs": [
      {
        "type": "text",
        "text": "What is 2 + 2? Return the answer as a number."
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
  }
  ```

  In this example, the agent declares that it requires an integer field named `answer`.  

  Because the schema is enforced at compile time, the LLM’s response must supply a valid integer — eliminating ambiguity at runtime.

  ### 2. JSON Logic for Conditional Actions

  After receiving the typed response, the agent applies **JSON Logic** rules to determine which actions to run.  
  (See: https://jsonlogic.com/)

  Here, the logic expression checks whether `answer` equals `4`.  
  If true, one command runs; if false, another:

  ```json
    "actions": [
        {
          "name": "is_4",
          "logic": {
            "==": [ { "var": "answer" }, 4 ]
          },
          "run": [
            {
              "kind": "exec",
              "program": "echo",
              "args": ["Value return is equal to 4."]
            }
          ]
        },
        {
          "name": "is_not_4",
          "logic": {
            "!=": [ { "var": "answer" }, 4 ]
          },
          "run": [
            {
              "kind": "exec",
              "program": "echo",
              "args": ["Value return is not equal to 4."]
            }
          ]
        }
      ]
  ```

### Why This Matters

Cargo‑AI gives you two powerful guarantees:

1. **Typed responses from any LLM**  
   Responses can include top-level integers, booleans, strings, and numbers, together with supported schema metadata such as `description`, string `enum`, and numeric bounds — all enforced through generated Rust types plus local validation.

2. **Full expressive power of JSON Logic**  
   Perform comparisons, branching, variable evaluation, and complex decision logic to drive arbitrary command‑line actions.

In short:  
**Now you can create sophisticated, predictable, atomic Rust agents — with no code.**


## 🌦️🤖 Create Your Own Weather Agent with JSON

We’ll walk through a [weather_test.json](./weather_test.json) example step-by-step—ordered inputs, expected response schema, and actions.

To define a custom agent, you’ll use a JSON file that specifies:
1. The ordered **inputs** to send to the AI/transformer server  
2. The **expected response schema** (properties returned)  
3. A set of **actions** to run, depending on the agent’s response  

The steps below show how to create the weather_test agent, but once defined, running it is as simple as:

```bash
# 1. Hatch your weather_test agent from a JSON config
cargo ai hatch weather_test --config weather_test.json

# 2. Run your weather_test agent using either your default profile or explicit flags
./weather_test
# or override the defaults:
./weather_test -s openai -m gpt-4o --token sk-ABCD1234...

# Expected output if raining tomorrow:
# bring an umbrella
```
> **Note for Windows users:**  
> Use `weather_test` (or `weather_test.exe`) instead of `./weather_test`.

### 1) Define the Inputs

  The `inputs` array is the ordered model-facing input the agent sends at runtime.  
  Start with `type: "text"` entries for plain instructions, and add `type: "url"` or `type: "image"` entries when the agent needs fetched web text or local image files.

  Example from [weather_test.json](./weather_test.json):

  ```json
  "inputs": [
    {
      "type": "text",
      "text": "Will it rain tomorrow between 9am and 5pm? (Consider true if over 40% for any given hour period.)"
    },
    {
      "type": "url",
      "url": "https://gettimeapi.dev/v1/time?timezone=UTC"
    },
    {
      "type": "url",
      "url": "https://api.open-meteo.com/v1/forecast?latitude=39.10&longitude=-84.51&hourly=precipitation_probability"
    }
  ]
  ```

  You can use multiple `text` inputs, multiple `url` inputs, and later local `image` inputs. Runtime flags such as `--input-text`, `--input-url`, and `--input-image` replace the config-defined inputs when present.

### 2) Define the Response Schema

  The `agent_schema` describes the shape of the response you expect from the AI/transformer server.  
  Behind the scenes, this schema is also used to generate the corresponding Rust structures.  

  You can define fields as:
  - `boolean` → true/false values  
  - `string` → text values  
  - `number` → floating-point numbers (f64)  
  - `integer` → whole numbers (i64)  

  Optional top-level property metadata and constraints:
  - `description` on any supported top-level field
  - `enum` on `string` fields only
  - `minimum`, `maximum`, `exclusiveMinimum`, and `exclusiveMaximum` on `number` and `integer` fields

  Current scope intentionally stays flat:
  - top-level arrays are rejected
  - nested objects and union types are rejected

  Example from [weather_test.json](./weather_test.json):

  ```json
  "agent_schema": {
    "type": "object",
    "properties": {
      "raining": {
        "type": "boolean",
        "description": "Indicates whether it is raining."
      }
    }
  }
   ```

  Example with enum and numeric bounds:

  ```json
  "agent_schema": {
    "type": "object",
    "properties": {
      "unit": {
        "type": "string",
        "description": "Temperature unit.",
        "enum": ["F", "C"]
      },
      "confidence": {
        "type": "number",
        "description": "Confidence score greater than 0 and less than or equal to 1.",
        "exclusiveMinimum": 0,
        "maximum": 1
      }
    }
  }
  ```

### 3) Define Actions

The `actions` section specifies what the agent should do based on the response.  
It follows the [JSON Logic](http://jsonlogic.com/) format for conditions.  

Currently, actions can run a command-line executable (`exec`).  
Future versions will support additional action types.

Action object schema:

```json
{
  "name": "my_action",
  "logic": { "==": [ { "var": "answer" }, 4 ] },
  "run": [
    {
      "kind": "exec",
      "program": "echo",
      "args": ["Value is 4"]
    }
  ]
}
```

- `name`: Action label shown in execution output/logs.
- `logic`: JSON Logic condition evaluated against the typed agent response.
- `run`: Ordered list of steps to execute when `logic` evaluates to true.

`run` step schema:

```json
{
  "platform": ["macos", "linux"],
  "kind": "exec",
  "program": "echo",
  "args": ["hello", { "var": "answer" }]
}
```

- `platform`: Optional OS selector. Use `macos`, `linux`, or `windows` as a string or array. Omit it to run the step on every runtime OS.
- `kind`: Step type. Use `"exec"` for command execution.
- `program`: Executable name or path to run.
- `args`: Argument tokens passed directly as argv entries (no shell splitting). Each entry may be a literal string or a `{ "var": "field_name" }` object that pulls from a top-level schema field at runtime.

Platform values are matched at the OS level, not by full target triple. For example, both Apple Silicon and Intel macOS builds match `macos`.
Variable args are limited to top-level scalar fields (`string`, `integer`, `number`, `boolean`). Array fields are not supported for arg substitution in this story.

Example from [weather_test.json](./weather_test.json):

```json
"actions": [
  {
    "name": "umbrella_hint_exec",
    "logic": {
      "==": [ { "var": "raining" }, true ]
    },
    "run": [
      {
        "platform": ["macos", "linux"],
        "kind": "exec",
        "program": "echo",
        "args": ["bring an umbrella because raining=", { "var": "raining" }]
      },
      {
        "platform": "windows",
        "kind": "exec",
        "program": "cmd",
        "args": ["/C", "echo", "bring an umbrella because raining=", { "var": "raining" }]
      }
    ]
  },
  {
    "name": "sunglasses_hint_exec",
    "logic": {
      "==": [ { "var": "raining" }, false ]
    },
    "run": [
      {
        "kind": "exec",
        "program": "echo",
        "args": ["bring sunglasses"]
      }
    ]
  }
]
```

In this example:
- If `raining` is true, the agent prints “bring an umbrella.”
- If `raining` is false, the agent prints “bring sunglasses.”
- Platformless steps run everywhere. Platform-targeted steps run only when the current runtime OS matches one of the configured platform values, and matching steps run in listed order.
- Variable args let a command consume the validated model output directly without shell expansion. `{ "var": "raining" }` resolves to the typed runtime value and is passed as a normal argv token.

---

`cargo-ai™` is an independent project and is not affiliated with, endorsed by, or sponsored by the Rust Foundation.
