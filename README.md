# cargo-ai

[![Audit Status](https://github.com/analyzer1/cargo-ai/actions/workflows/security-audit.yml/badge.svg)](https://github.com/analyzer1/cargo-ai/actions/workflows/security-audit.yml)
[![Warning: Under Active Development](https://img.shields.io/badge/Warning-Under_Active_Development-yellow)](https://github.com/analyzer1/cargo-ai)

## 🌐 Overview
`cargo-ai` is a Cargo subcommand and Rust library for integrating AI models into your workflow.

## ✨ Features

- CLI and library for interacting with AI models in Rust

## 📦 Installation

**Prerequisites**: Ollama server running on its default local IP address and the Mistral open-source model available.

Install the CLI and library from source:

```bash
cargo install cargo-ai
cargo ai --server ollama --model mistral
```

Sample output:

```text
Enter a prompt for mistral!
4+5
ollama Response: 9
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