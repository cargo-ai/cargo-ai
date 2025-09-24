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

### Create an Agent

1. **Hatch a Hello World agent** (AdderAgent adds 2 + 2 by default):  

   Generic form:  
   ```bash
   cargo ai hatch <YourAgentName>
   ```

   Example:  
   ```bash
   cargo ai hatch AdderAgent
   ```

## ⚙️ CLI Usage

```bash
Usage: cargo ai [OPTIONS] --server <server> --model <model>

Options:
  -s, --server <server>       Client Type – Ollama or OpenAI
  -m, --model <model>         LLM model to use
  --token <token>             API token
  --timeout_in_sec <timeout>  Client timeout request [default: 60]
  -h, --help                  Print help
```