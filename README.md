# cargo-ai

[![Audit Status](https://github.com/analyzer1/cargo-ai/actions/workflows/security-audit.yml/badge.svg)](https://github.com/analyzer1/cargo-ai/actions/workflows/security-audit.yml)
[![Warning: Under Active Development](https://img.shields.io/badge/Warning-Under_Active_Development-yellow)](https://github.com/analyzer1/cargo-ai)

## 🌐 Overview
`cargo-ai` is a lightweight, Rust-based framework for building **no-code AI agents** using clean, declarative, JSON configs. Agents compile into fast, secure binaries—perfect for local machines, servers, and embedded Linux devices, with broader embedded support planned.  

*Lightweight AI agents. Built in Rust. Declared in JSON.*

## ✨ Features

- **Declarative, No-Code Agents** – Define agent logic in JSON  
- **Rust-Powered** – Safe, fast, and portable across environments  
- **Compile-Time Safety** – Minimal runtime overhead; standalone binaries  
- **Fully Local & Secure** – All logic executes client-side (no phoning home)  
- **Embedded-Ready** – Agents compile into binaries suitable for servers and embedded Linux devices, with broader embedded support planned  
- **Composable CLI** – Scaffold, build, and run agents via `cargo ai` commands  

## 📦 Installation

### Base Install

1. **Install Rust & Cargo**  
   Follow the official guide:  
   [Install Rust & Cargo](https://doc.rust-lang.org/cargo/getting-started/installation.html)

2. **Install cargo-ai**  
   Once Cargo is available, install `cargo-ai` from source:  
   ```bash
   cargo install cargo-ai
   ```

    Verify installation:  
    ```bash
    cargo ai --help
    ```

### Create a Sample Agent

1. **Hatch a Sample Agent** (AdderAgent – a sample “Hello World” style agent that adds 2 + 2):  

   Generic form:  
   ```bash
   cargo ai hatch <YourAgentName>
   ```

   Example:  
   ```bash
   cargo ai hatch AdderAgent
   ```

### Run the Sample Agent

2. **Run the compiled agent** with OpenAI GPT:  

   Generic form:  
   ```bash
   ./<YourAgentName> -s <server> -m <model> --token <your_api_token>
   ```

   Example (AdderAgent with GPT-4o):  
   ```bash
     ./AdderAgent -s openai -m gpt-4o --token sk-ABCD1234...
   ```

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

### Agent Commands

Once hatched, your agent is compiled as a standalone binary.  
Example with `AdderAgent` (binary name: `AdderAgent`):

```bash
Usage: AdderAgent [OPTIONS]

Options:
  -s, --server <server>       Client Type – Ollama or OpenAI
  -m, --model <model>         LLM model to use
  --token <token>             API token
  --timeout_in_sec <timeout>  Client timeout request [default: 60]
  -h, --help                  Print help
```

## 🌦️🤖 Create Your Own Weather Agent with JSON

We’ll walk through a [WeatherAgent.json](./WeatherAgent.json) example step-by-step—prompt, expected response schema, optional resource URLs, and actions.

To define a custom agent, you’ll use a JSON file that specifies:
1. The **prompt** to send to the AI/transformer server  
2. The **expected response schema** (properties returned)  
3. (Optional) **Resource URLs** provided to the server alongside the prompt  
4. A set of **actions** to run, depending on the agent’s response  

The steps below show how to create the WeatherAgent, but once defined, running it is as simple as:

```bash
# 1. Hatch your WeatherAgent from a JSON config
cargo ai hatch WeatherAgent [WeatherAgent.json](./WeatherAgent.json)

# 2. Run your WeatherAgent with a server, model, and token
./WeatherAgent -s openai -m gpt-4o --token sk-ABCD1234...

# Expected output if raining tomorrow:
# bring an umbrella
```

### 1) Define the Prompt

  The `prompt` is the natural language instruction or question you send to the AI/transformer server.  
  It frames what the agent is supposed to do. You can phrase it as a question, a request, or a directive.

  Example from [WeatherAgent.json](./WeatherAgent.json):

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

  Example from [WeatherAgent.json](./WeatherAgent.json):

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

  Example from [WeatherAgent.json](./WeatherAgent.json):

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

Example from [WeatherAgent.json](./WeatherAgent.json):

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