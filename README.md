# cargo-ai

[![Audit Status](https://github.com/analyzer1/cargo-ai/actions/workflows/security-audit.yml/badge.svg)](https://github.com/analyzer1/cargo-ai/actions/workflows/security-audit.yml)
[![Warning: Under Active Development](https://img.shields.io/badge/Warning-Under_Active_Development-yellow)](https://github.com/analyzer1/cargo-ai)

## 🌐 Overview
`cargo-ai` is a lightweight, Rust-based framework for building **no-code AI agents** using clean, declarative JSON configs. Agents compile into fast, secure binaries—perfect for local machines, servers, and embedded Linux devices, with broader embedded support planned.  

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

1. **Hatch a Sample Agent** (AgentAdder – a sample “Hello World” style agent that adds 2 + 2):  

   Generic form:  
   ```bash
   cargo ai hatch <YourAgentName>
   ```

   Example:  
   ```bash
   cargo ai hatch AgentAdder
   ```

### Run the Sample Agent

2. **Run the compiled agent** with OpenAI GPT:  

   Generic form:  
   ```bash
   ./<YourAgentName> -s <server> -m <model> --token <your_api_token>
   ```

   Example (AgentAdder with GPT-4o):  
   ```bash
     ./AgentAdder -s openai -m gpt-4o --token sk-ABCD1234...
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
Example with `AgentAdder` (binary name: `AgentAdder`):

```bash
Usage: AgentAdder [OPTIONS]

Options:
  -s, --server <server>       Client Type – Ollama or OpenAI
  -m, --model <model>         LLM model to use
  --token <token>             API token
  --timeout_in_sec <timeout>  Client timeout request [default: 60]
  -h, --help                  Print help
```