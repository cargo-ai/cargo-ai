# Install Cargo AI With Cargo

This is the preferred install path today.

It gives you:

- the full Cargo AI CLI
- the normal `cargo ai ...` workflow
- direct `cargo-ai ...` invocation when Cargo's bin directory is on `PATH`
- access to native-export commands such as `hatch`

## 1. Install Rust and Cargo

If Rust and Cargo are not already installed, use the official `rustup` guide:

- [Install Rust](https://rust-lang.org/tools/install/)

Verify Cargo is available:

```bash
cargo --version
```

## 2. Install Cargo AI

```bash
cargo install cargo-ai
```

Use the current stable Rust toolchain for installation. Cargo resolves compatible dependency versions from the package manifest. Cargo AI's release checks exercise both the packaged dependency lockfile and a freshly resolved dependency graph.

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
cargo install cargo-ai
```

Cargo AI uses a product-oriented pre-`1.0.0` release policy:

- `0.y.0` means meaningful product or contract evolution
- `0.y.z` is reserved for smaller fixes and polish

See [Versioning](../../VERSIONING.md) for the public versioning policy.

For `0.3.x → 0.4.0`, review the [release notes](../../releases/0.4.0.md) for definition and command migration. Existing generated agents may report that they are out of sync with local Cargo AI metadata until they are re-hatched.

### Optional dependency reproduction

If an installation fails after dependency updates, preserve the error and toolchain version. To try the dependency versions packaged with a particular release, use:

```bash
cargo install cargo-ai --version 0.4.0 --locked
```

This is an optional troubleshooting path. It fixes the dependency selection to that release's lockfile, so later dependency fixes are not selected automatically. It does not replace the normal install or upgrade command above. See [Cargo's lockfile behavior](https://doc.rust-lang.org/cargo/commands/cargo-install.html#dealing-with-the-lockfile).

## 5. Local State

By default, Cargo AI stores local config, credentials, and internal workspaces under Cargo AI Home.

See [Cargo AI Home](../cargo-ai-home.md) for:

- resolution order
- stored files and directories
- first-run initialization behavior
- `CARGO_AI_HOME` overrides

## Next

- [Build and run your first agent](../getting-started.md)
- [Choose a model provider](../providers/README.md)
- [Documentation home](../README.md)
- [Public README](../../README.md)
