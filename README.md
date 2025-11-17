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
- **LLM Connection Profiles** – Store reusable settings for servers, models, tokens, and timeouts so you don't re-enter them each run
- **Repository Integration** – Download JSON configurations directly from Cargo-AI and hatch agents without needing local files
- **Cross‑Platform Support** – Runs on any Linux, macOS, or Windows device

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

### Configure an LLM Profile (Recommended)

Before hatching or running agents, it is recommended to set up a default LLM connection profile.  
This allows `cargo-ai` to run agents without requiring server, model, or token flags each time.

#### Add a Default OpenAI Profile

Example (using OpenAI GPT 4o):

```bash
cargo ai profile add openai \
    --server openai \
    --model gpt-4o \
    --token sk-*** \
    --default
```

Cargo-AI supports Ollama and OpenAI‑compatible transformer servers. To change the default URL, use:
```bash
--url <custom_llm_endpoint>
```

### Create a Sample Agent

1. **Hatch a Sample Agent**  

   By default, if you don’t provide a config file, `cargo-ai` will hatch a sample “Hello World” style agent (`adder_test`) that simply adds 2 + 2.

   Default example:  
   ```bash
   cargo ai hatch adder_test
   ```

   Generic form (using your own config file):  
   ```bash
   cargo ai hatch <YourAgentName> -c <config_file>
   ```

### Run the Sample Agent

2. **Run the compiled agent** with OpenAI GPT:  

    Generic form:  
    ```bash
    ./<YourAgentName> -s <server> -m <model> --token <your_api_token>
    ```

    Example (adder_test with GPT-4o):  
    ```bash
    ./adder_test -s openai -m gpt-4o --token sk-ABCD1234...
    ```

    #### Override the Default Prompt

    By default, the `adder_test` runs with the prompt `"2+2"`. You can override this at runtime using the `--prompt` flag.

    Default run (prompt = "2+2"):  
    ```bash
    ./adder_test -s openai -m gpt-4o --token sk-ABCD1234...
    ```

    Expected output:
    ```
    Running 'is_4': echo ["Value return is equal to 4."]
    Value return is equal to 4.
    Command completed successfully.
    ```

    Overridden run (prompt = "2+3"):  
    ```bash
    ./adder_test -s openai -m gpt-4o --token sk-ABCD1234... --prompt "2+3"
    ```

    Expected output:
    ```
    Running 'is_not_4': echo ["Value return is not equal to 4."]
    Value return is not equal to 4.
    Command completed successfully.
    ```
    > **Note for Windows users:**  
    > On Windows, the agent binary will be created with a `.exe` extension (e.g., `adder_test.exe`).  
    > You can run it by simply typing `adder_test` in PowerShell or Command Prompt (the `.exe` is implied).  
    > On macOS and Linux, run the binary from the current directory using `./adder_test`.

## ⚙️ CLI Usage

### Cargo AI Commands

The base `cargo ai` command provides subcommands for managing agents:

```bash
Usage: cargo ai [COMMAND]

Commands:
  hatch    Hatch a new AI agent project
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help   Print help
```

#### Hatch Command

The `hatch` command creates a new AI agent from a JSON config:

```bash
Usage: cargo ai hatch [OPTIONS] <name>

Arguments:
  <name>  Name of the new agent project

Options:
  -c, --config <FILE>  Path to the agent configuration file (JSON format)
  -h, --help           Print help
```

### Agent Commands

Once hatched, your agent is compiled as a standalone binary.  
Example with `adder_test` (binary name: `adder_test`):

```bash
Usage: adder_test [OPTIONS]

Options:
  -s, --server <server>       Client Type – Ollama or OpenAI
  -m, --model <model>         LLM model to use
  --token <token>             API token
  --timeout_in_sec <timeout>  Client timeout request [default: 60]
  -p, --prompt <TEXT>         Prompt to provide to the agent at runtime
  -h, --help                  Print help
```

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

# 2. Run your weather_agent with a server, model, and token
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
      "url": "https://worldtimeapi.org/api/timezone/etc/utc",
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