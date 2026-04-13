# Cargo AI Home

`Cargo AI Home` is the local state root for Cargo AI.

Cargo AI stores local configuration, credentials, and internal workspaces under this directory so the same installation can:

- run JSON agents directly
- manage local profiles and credentials
- hatch/check agents when the fuller developer toolchain is available

## Resolution Order

Cargo AI resolves its home directory in this order:

1. `CARGO_AI_HOME`
2. `CARGO_HOME/.cargo-ai`
3. `~/.cargo/.cargo-ai`

If neither `CARGO_AI_HOME` nor `CARGO_HOME` is set, Cargo AI still uses the Cargo-compatible default under your home directory.

## What Lives There

Cargo AI Home may contain:

- `config.toml`
- `credentials.toml`
- `agents/`
- `templates/`
- `locks/`

These cover local profile/config state, credential storage, hatched/check workspaces, warmed build templates, and lock files used during local build/export flows.

## Why The Default Stays Under `.cargo`

The default stays Cargo-compatible on purpose.

That lets users move between:

- standalone `cargo-ai` usage
- later `cargo install cargo-ai --locked`
- later full developer/export workflows

without splitting Cargo AI state across different directories.

## First Run

On first run, Cargo AI may create Cargo AI Home automatically if it does not exist yet.

Cargo AI now prints a one-time initialization notice when it creates that directory so the location is visible instead of silent.

## Override It

Set `CARGO_AI_HOME` if you want Cargo AI to use a different local state root:

```bash
export CARGO_AI_HOME="$HOME/.cargo-ai"
```

Example:

```bash
CARGO_AI_HOME="$HOME/.cargo-ai" cargo-ai run ./agent.json
```

## Installed With Cargo vs Standalone Binary

This behavior is the same whether you:

- install Cargo AI with `cargo install`
- run a standalone `cargo-ai` binary directly

The difference is only how the executable is installed and discovered on `PATH`. The local state root resolution stays the same unless you override it with `CARGO_AI_HOME`.
