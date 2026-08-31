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

## Isolate Development And Test Runs

Use a dedicated Cargo AI Home whenever you run a locally built binary. Keep the
development home outside your normal Cargo directory and any `.cargo-ai*`
recovery directories.

From a Cargo AI source checkout:

```bash
export CARGO_AI_HOME="/path/to/disposable/cargo-ai-home"
export CARGO_AI_DISABLE_KEYCHAIN=1
cargo run -- version
```

For reinstall testing, isolate the executable as well as its state:

```bash
cargo install --path . --root /path/to/disposable/cargo-ai-install --force
CARGO_AI_HOME="/path/to/disposable/cargo-ai-home" \
  CARGO_AI_DISABLE_KEYCHAIN=1 \
  /path/to/disposable/cargo-ai-install/bin/cargo-ai version
```

Do not populate a development home by copying live credentials. Create only the
profiles and test state that the development scenario requires. Disabling the
keychain matters because operating-system keychain entries are not namespaced
by `CARGO_AI_HOME`.

## Failure-Safe Automatic State Updates

Cargo AI distinguishes a missing `config.toml` from an existing file that it
cannot read or parse. A missing config may be initialized. An unreadable or
malformed config produces a path-specific warning and blocks automatic
credential migration, metadata persistence, and update-check persistence for
that invocation. Cargo AI does not treat the failure as permission to replace
the file with defaults.

For a valid config, automatic writers:

- update only the fields they own while preserving unrecognized fields
- skip the write when the owned values are already current
- stage and validate replacement TOML beside the active file
- use owner-only file permissions on Unix
- retain a `config.toml.bak` recovery copy only when the prior valid state
  contains known non-secret fields

Unrecognized fields always remain in the active config. Cargo AI omits the
managed backup when it cannot prove that every copied field is non-secret.
During startup reconciliation, Cargo AI preserves an existing exact
`config.toml.bak` only when it strictly validates as fully known and
non-secret; an unsafe or unverifiable managed backup may be removed.

One-time legacy credential migration persists detected credentials before it
replaces `config.toml` with a validated scrubbed copy. It does not create a
pre-scrub backup containing legacy tokens. If credential persistence or config
replacement fails, the original config remains available and later automatic
writers are skipped for that invocation.

Cargo AI manages only the active home and its `config.toml.bak`. It does not
discover, import, rotate, rename, or delete sibling `.cargo-ai*` directories
that you maintain as recovery copies.

## Installed With Cargo vs Standalone Binary

This behavior is the same whether you:

- install Cargo AI with `cargo install`
- run a standalone `cargo-ai` binary directly

The difference is only how the executable is installed and discovered on `PATH`. The local state root resolution stays the same unless you override it with `CARGO_AI_HOME`.

## Related Docs

- [Install Cargo AI](./install/README.md)
- [Build and run your first agent](./getting-started.md)
- [Packages and installed data](./packages.md)
- [Documentation home](./README.md)
- [Public README](../README.md)
