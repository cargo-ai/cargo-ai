# Troubleshoot Cargo AI

[Documentation hub](./README.md) · [Project README](../README.md)

Start with the command that failed, preserve its exact error, and fix the narrowest cause before changing the agent or provider contract. Cargo AI generally fails closed when a profile, schema, path, permission, or provider capability is invalid.

For exhaustive definition-validation errors, use the generated [offline troubleshooting guide](../templates/guidance/troubleshooting.md). This page covers the surrounding install, runtime, profile, package, and operating-system issues that belong in the human documentation journey.

## The Definition Does Not Validate

Run the check loop against the exact file you plan to hatch:

```bash
cargo ai hatch my_agent --config ./my_agent.json --check
```

`--check` validates scaffold and compile behavior with `cargo check` but does not export a binary. Fix the first reported field path, repeat the command, and build only after it passes.

Common causes include:

- a missing required top-level key or the rejected legacy `version` key
- a field attached to the wrong run-step kind
- an undeclared or out-of-scope variable reference
- an absolute path or parent traversal (`..`)
- runtime inputs replacing a baked instruction unexpectedly
- a named input that was never satisfied with `--input-override`
- a platform selector that skips the step on the current operating system

Read [Agent Definitions](./agent-definitions.md) and [Actions And Child Agents](./actions-and-child-agents.md) for the human overview. Use the generated [agent definition contract](../templates/guidance/agent-definition-contract.md) and [action rules](../templates/guidance/action-rules.md) for exhaustive accepted fields.

## Cargo AI Is Not Found Or Is Out Of Date

Verify the install:

```bash
cargo ai --help
```

Cargo installs the executable as `cargo-ai`; `cargo ai ...` works when Cargo can find that executable on `PATH`. If neither form works, follow [Install Cargo AI](./install/README.md), including the platform-specific Cargo bin directory guidance.

Upgrades are manual:

```bash
cargo install cargo-ai --locked
```

After a meaningful pre-`1.0.0` upgrade, re-hatch generated agents whose embedded metadata is out of sync. See [Install With Cargo](./install/cargo.md) and [Versioning](../VERSIONING.md).

## A Profile Fails Or The Wrong Provider Runs

`--profile <name>` is strict. If that profile is missing or invalid, Cargo AI fails instead of falling back to another profile or profileless authentication.

Generated binaries otherwise use the configured/default profile unless runtime flags override it. Check the root `using:` line for the effective profile, authentication mode, server, and model. A URL appears only when it is custom or materially different from the standard transport.

For provider credentials, capability limits, supported input types, and setup commands, use [Provider Setup](./providers/README.md). Store real API keys in a profile; do not place them in agent JSON or shell arguments.

Standalone provider notes:

- OpenAI account authentication can reuse an available local Codex session with `--server openai --model <model>` and no token.
- Anthropic requires an `api_key` profile with its key stored through `cargo ai profile set --stdin` or `--env`.
- Gemini requires an `api_key` profile with its key stored through `cargo ai profile set --stdin` or `--env`.

Provider pages remain the authority for current endpoint and feature differences. Cargo AI surfaces unsupported input, image-generation, or schema behavior instead of silently changing providers or weakening the authored contract.

## A Standalone Agent Works Only On The Author's Machine

A standalone recipient does not need Cargo AI installed when the binary has no package-child (`alias::entrypoint`) references and the recipient supplies the required runtime context through a configured profile, such as:

```bash
./my_agent \
  --profile <profile> \
  --render-mode append-only
```

Profileless server, model, URL, and token flags depend on the provider and authentication mode. The runtime accepts a token flag for compatible profileless use, but placing a real secret in a command exposes it to shell history and potentially process inspection. Use a stored profile on machines you control.

Package-child references are different: the hatched binary requires `cargo ai` or `cargo-ai` on `PATH` so Cargo AI can enforce installed package identity, version, and permission policy. It also fails closed outside a Cargo AI project when that project context is required. Read [Packages](./packages.md) for the full boundary.

## A Child Agent Or Action Did Not Behave As Expected

Check these boundaries before adding a wrapper script:

- child targets use an explicit relative artifact such as `./child.json`
- named parent inputs are forwarded explicitly rather than inherited
- `input_mode` controls anonymous child inputs, not named overrides
- the parent captures child success/failure with status and error variables
- child structured output is not automatically merged into the parent
- `when` and `platform` skips leave outcome variables unset
- action failure modes govern the remaining steps in that action

Use [Actions And Child Agents](./actions-and-child-agents.md) for scheduling, depth/runtime limits, failure behavior, rendering, and usage logs.

## A Package Entrypoint Does Not Run

Do not work around package failures by copying artifacts or weakening permissions. Inspect the installed alias, declared version/source binding, accepted permissions, and project context.

Cross-package child execution may require an accepted subprocess permission. Hosted references never silently bind an unrelated local alias, and installed package execution does not fall back to source when a runtime artifact is missing or invalid.

Follow [Packages](./packages.md) for install, repair, permissions, persistent data, hosted identity, and archive limits. Follow [Projects And Local Tools](./projects-and-tools.md) when the failure involves a source-backed tool or project manifest.

## `version` And `inspect` Report Different Things

On a machine without Cargo AI installed or configured, `./my_agent version` reports embedded version/provenance but treats comparison with local Cargo AI state as not checked. Use:

```bash
./my_agent inspect
```

to view the embedded agent provenance. Re-hatch after upgrading Cargo AI when the generated executable should incorporate the newer local templates or metadata.

## I Need To Schedule An Agent

Scheduling is not built into Cargo AI. Use the operating system scheduler around the normal command:

- `cron` or another service manager on macOS/Linux
- Task Scheduler on Windows

Keep profiles, working directories, package context, input paths, and output paths explicit because scheduled processes usually have a smaller environment than an interactive shell.

## The Problem Persists

Collect only the non-secret context needed to reproduce the failure:

- the exact Cargo AI command with credentials redacted
- the first complete error and field path
- `cargo ai --help` availability and the relevant command help
- operating system and invocation directory
- whether the source is local JSON, inline/stdin, registry-backed, or hatched
- the root `using:` line with tokens and credentials omitted
- package alias/source/version information when a package is involved

Do not share Cargo AI Home credential files, API keys, access tokens, raw provider responses, or unredacted usage logs. Usage ledgers intentionally omit prompts, outputs, tool arguments, and secrets, but review any artifact before sharing it.

## Related Documentation

- [Documentation hub](./README.md)
- [Project README](../README.md)
- [Getting Started](./getting-started.md)
- [Provider Setup](./providers/README.md)
- [Install Cargo AI](./install/README.md)
- [Cargo AI Home](./cargo-ai-home.md)
- [Packages](./packages.md)
- [Projects And Local Tools](./projects-and-tools.md)
