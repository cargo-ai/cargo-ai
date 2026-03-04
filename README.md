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
    --default
```

Set profile auth mode and token-based credentials:

```bash
cargo ai profile auth set openai api_key
cargo ai profile token set openai --token sk-***
```

Or use OpenAI account login instead of API key:

```bash
cargo ai auth login openai --profile openai --set-default
cargo ai profile auth set openai openai_account
```

`cargo ai auth login openai` follows OpenAI/Codex browser sign-in semantics and reads session tokens from Codex local auth storage (`$CODEX_HOME/auth.json`, or `~/.codex/auth.json` by default) without importing duplicate OpenAI account tokens into Cargo AI secret stores.

Cargo-AI supports Ollama and OpenAI‑compatible transformer servers. To change the default URL, use:
```bash
--url <custom_llm_endpoint>
```

#### Credential Storage (Phase 1)

- `config.toml` is metadata-only and no longer persists profile/account secret values.
- Default secret-store mode is `file` for new installs.
- `file` mode stores secrets in `credentials.toml` at:
  - `$CARGO_HOME/.cargo-ai/credentials.toml`, or
  - `~/.cargo/.cargo-ai/credentials.toml`.
- `keychain` mode stores secrets in OS keychain backends through Rust `keyring` (3.x stable).
- Manage mode with:
  - `cargo ai settings secret-store status`
  - `cargo ai settings secret-store set <file|keychain>`
  - `cargo ai settings secret-store set <file|keychain> --migrate --yes`
- Manage OpenAI auth session with:
  - `cargo ai auth login openai [--profile <name>] [--set-default]`
  - `cargo ai auth status [--json]`
  - `cargo ai auth logout [--global] [--yes]`
- `cargo ai auth logout` is local-only by default (Cargo AI stops using OpenAI account auth, Codex remains signed in).
- Use `cargo ai auth logout --global` to also run `codex logout`.
- Manage profile auth/token material with:
  - `cargo ai profile auth set <name> <none|api_key|openai_account>`
  - `cargo ai profile auth status [<name>]`
  - `cargo ai profile token set <name> (--token <TOKEN> | --stdin | --env <ENV_VAR>)`
  - `cargo ai profile token clear <name> [--yes]`
  - `cargo ai profile token status <name>`
- Use `--migrate --dry-run` to preview migration without writes.
- Fresh installs can switch to `keychain` before any secrets are created (metadata-only switch).
- Legacy secrets found in `config.toml` are migrated once at startup into the active secret-store path.
- Generated agents use the same secret-store mode behavior, so default-profile token usage remains compatible.

### Create a Sample Agent

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

### Hatch from Account Agents

When you are signed in, you can hatch an agent directly from account-hosted definitions:

```bash
# Hatch your own account agent (path defaults to "/")
cargo ai account agents hatch weather_agent

# Override the local output/workspace name
cargo ai account agents hatch weather_agent --local-name weather_agent_v2

# Overwrite existing local output binary
cargo ai account agents hatch weather_agent --force
```

To hatch a public agent from another owner:

```bash
cargo ai account agents hatch weather_agent --owner-handle alice
```

To select a non-root definition path:

```bash
cargo ai account agents hatch weather_agent --definition-path /team/ops
```

  To understand what is happening behind the scenes, we can look at the internal structure of the sample agent JSON file, [`adder_test.json`](./adder_test.json). 

  ### 1. Prompt and Guaranteed Typed Response

  Each agent defines a natural‑language **prompt** together with a strongly‑typed **response schema**.  
  
  The schema is compiled into Rust types, guaranteeing that the agent will always receive data in the expected shape.

  ```json
  {
    "prompt": "What is 2 + 2? Return the answer as a number.",
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
   Responses can include integers, booleans, strings, numbers, and arrays of these types — all enforced at compile time.

2. **Full expressive power of JSON Logic**  
   Perform comparisons, branching, variable evaluation, and complex decision logic to drive arbitrary command‑line actions.

In short:  
**Now you can create sophisticated, predictable, atomic Rust agents — with no code.**


## 🌦️🤖 Create Your Own Weather Agent with JSON

We’ll walk through a [weather_agent.json](./weather_agent.json) example step-by-step—prompt, expected response schema, optional resource URLs, and actions.

To define a custom agent, you’ll use a JSON file that specifies:
1. The **prompt** to send to the AI/transformer server  
2. The **expected response schema** (properties returned)  
3. (Optional) **Resource URLs** provided to the server alongside the prompt  
4. A set of **actions** to run, depending on the agent’s response  

The steps below show how to create the weather_agent, but once defined, running it is as simple as:

```bash
# 1. Hatch your weather_agent from a JSON config
cargo ai hatch weather_agent --config weather_agent.json

# 2. Run your weather_agent using either your default profile or explicit flags
./weather_agent
# or override the defaults:
./weather_agent -s openai -m gpt-4o --token sk-ABCD1234...

# Expected output if raining tomorrow:
# bring an umbrella
```
> **Note for Windows users:**  
> Use `weather_agent` (or `weather_agent.exe`) instead of `./weather_agent`.

### 1) Define the Prompt

  The `prompt` is the natural language instruction or question you send to the AI/transformer server.  
  It frames what the agent is supposed to do. You can phrase it as a question, a request, or a directive.

  Example from [weather_agent.json](./weather_agent.json):

  ```json
  "prompt": "Will it rain tomorrow between 9am and 5pm? (Consider true if over 40% for any given hour period.)"
  ```

  You can edit the text to suit your agent’s purpose—for example, summarizing an article, checking stock prices, or answering domain-specific questions.

### 2) Define the Response Schema

  The `agent_schema` describes the shape of the response you expect from the AI/transformer server.  
  Behind the scenes, this schema is also used to generate the corresponding Rust structures.  

  You can define fields as:
  - `boolean` → true/false values  
  - `string` → text values  
  - `number` → floating-point numbers (f64)  
  - `integer` → whole numbers (i64)  

  Example from [weather_agent.json](./weather_agent.json):

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

### 3) Define Resource URLs

  The `resource_urls` section lists optional external data sources your agent can use.  
  Each entry includes:
  - `url`: the API endpoint or resource location  
  - `description`: a short explanation of what the resource provides  

  These URLs are passed to the AI/transformer server alongside the prompt, giving the agent more context to work with.  

  Example from [weather_agent.json](./weather_agent.json):

  ```json
  "resource_urls": [
    {
      "url": "https://gettimeapi.dev/v1/time?timezone=UTC",
      "description": "Current UTC date and time."
    },
    {
      "url": "https://api.open-meteo.com/v1/forecast?latitude=39.10&longitude=-84.51&hourly=precipitation_probability",
      "description": "Hourly precipitation probability for Cincinnati, which is my area."
    }
  ]
  ```

  *Note: The weather forecast URL in the example is configured for Cincinnati (latitude/longitude values). Update these values and the description to match your own location.*

### 4) Define Actions

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
  "kind": "exec",
  "program": "echo",
  "args": ["hello", "world"]
}
```

- `kind`: Step type. Use `"exec"` for command execution.
- `program`: Executable name or path to run.
- `args`: Argument tokens passed directly as argv entries (no shell splitting).

Example from [weather_agent.json](./weather_agent.json):

```json
"actions": [
  {
    "name": "umbrella_hint_exec",
    "logic": {
      "==": [ { "var": "raining" }, true ]
    },
    "run": [
      {
        "kind": "exec",
        "program": "echo",
        "args": ["bring an umbrella"]
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
