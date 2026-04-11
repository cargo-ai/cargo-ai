# Install Cargo AI With Cargo

This is the preferred install path today.

It gives you:

- the full Cargo AI CLI
- the normal `cargo ai ...` workflow
- direct `cargo-ai ...` invocation when Cargo's bin directory is on `PATH`
- access to developer-oriented commands such as `hatch`

## 1. Install Rust and Cargo

If Rust and Cargo are not already installed, use the official `rustup` guide:

- [Install Rust](https://rust-lang.org/tools/install/)

Verify Cargo is available:

```bash
cargo --version
```

## 2. Install Cargo AI

```bash
cargo install cargo-ai --locked
```

Verify the install:

```bash
cargo ai --help
```

## 3. Understand `cargo ai` vs `cargo-ai`

After `cargo install`, Cargo places the `cargo-ai` executable in Cargo's bin directory.

- macOS / Linux default: `~/.cargo/bin`
- Windows default: `%USERPROFILE%\\.cargo\\bin`

`cargo ai ...` works because Cargo discovers an executable named `cargo-ai` on `PATH` and dispatches to it as a Cargo subcommand.

`cargo-ai ...` works directly when Cargo's bin directory is on `PATH`.

Many `rustup` installations already configure this, but if direct `cargo-ai` invocation is not found, add Cargo's bin directory to your shell or user `PATH`.

## 4. Upgrade Cargo AI

Upgrades remain manual:

```bash
cargo install cargo-ai --locked
```

Cargo AI uses a product-oriented pre-`1.0.0` release policy:

- `0.y.0` means meaningful product or contract evolution
- `0.y.z` is reserved for smaller fixes and polish

See [Versioning](../../VERSIONING.md) for the public versioning policy.

After a meaningful pre-`1.0.0` upgrade such as `0.1.0 -> 0.2.0`, generated agents may report that they are out of sync with local Cargo AI metadata until they are re-hatched.

## 5. Local State

By default, Cargo AI stores local config, credentials, and internal workspaces under Cargo AI Home.

See [Cargo AI Home](../cargo-ai-home.md) for:

- resolution order
- stored files and directories
- first-run initialization behavior
- `CARGO_AI_HOME` overrides
