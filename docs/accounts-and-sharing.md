# Accounts and sharing

[Documentation hub](./README.md) · [Cargo AI README](../README.md)

A Cargo AI account is optional. Local `run`, `hatch`, project, build, and package workflows do not require registration. Register when you want hosted agent-definition storage, public sharing through a handle, account email workflows, or hosted package publishing.

Hosted agent management uses `cargo ai agents`. Running and hatching those definitions remain top-level `cargo ai run` and `cargo ai hatch` commands.

## Register and confirm an account

Register an email address, then confirm the temporary code sent to it:

```bash
cargo ai account register you@example.com
cargo ai account confirm <code-from-email>
cargo ai account status
```

Cargo-AI.org assigns a handle automatically. You can inspect it or choose a different available handle:

```bash
cargo ai account handle
cargo ai account handle --set your-handle
```

A handle is the public owner identity used when other people list, pull, run, or hatch definitions you share. It is not a local filesystem path.

## Manage hosted agent definitions

Agent storage and visibility commands live under the top-level `agents` group:

```bash
# List your active agents or another owner's public agents.
cargo ai agents list
cargo ai agents list --owner-handle alice

# Include your own archived agents or request a larger listing.
cargo ai agents list --include-archived
cargo ai agents list --limit 50
cargo ai agents list --all

# Upload a local definition. The name may be inferred from the file name.
cargo ai agents push --json-file ./weather_test.json --name weather_test
cargo ai agents push ./weather_test.json
```

`push` accepts exactly one definition source: `--json`, `--json-file`, or a positional file. Raw `--json` requires `--name`; file input can infer the name.

Pull your own definition or a public definition from another owner:

```bash
cargo ai agents pull weather_test
cargo ai agents pull weather_test --owner-handle alice
cargo ai agents pull weather_test --stdout
cargo ai agents pull weather_test --json-file ./saved/weather.json
```

Without `--stdout` or `--json-file`, `pull` writes `./<name>.json`. It refuses to overwrite an existing output unless you pass `--force`. `--stdout` performs no default file write.

Definitions may also use an account-side namespace:

```bash
cargo ai agents push --json-file ./weather_test.json \
  --name weather_test \
  --definition-path /agents/demos

cargo ai agents pull weather_test \
  --owner-handle alice \
  --definition-path /agents/demos
```

`--definition-path` is an account-side namespace path, not a local filesystem path.

## Visibility and archive state

New or private definitions are not shared through another owner's handle until visibility is changed:

```bash
cargo ai agents visibility --name weather_test --public
cargo ai agents visibility --name weather_test --private
```

Visibility changes are immediate and remain in effect until another explicit `--public` or `--private` update.

Archive state is separate from visibility:

```bash
cargo ai agents archive --name weather_test --archive
cargo ai agents archive --name weather_test --unarchive
```

Archived definitions are omitted from your normal list. Use `cargo ai agents list --include-archived` when you need to find or restore one.

## Run or hatch a hosted definition

Management remains under `cargo ai agents`; execution does not. Run or hatch your authenticated account definition with top-level commands:

```bash
cargo ai run weather_test --from-account --profile my_profile
cargo ai hatch weather_test --from-account
cargo ai hatch weather_test --from-account --check
```

Use an owner handle for another person's public definition:

```bash
cargo ai run weather_test --owner-handle alice --profile my_profile
cargo ai hatch weather_test --owner-handle alice
```

For a namespaced definition, add the same account-side path:

```bash
cargo ai run weather_test \
  --owner-handle alice \
  --definition-path /agents/demos \
  --profile my_profile
```

`--from-account` selects your authenticated account. `--owner-handle <handle>` selects another owner's public definition. They are mutually exclusive and replace local/registry name resolution for that invocation.

Hatching accepts `--agent <remote-name>` when the desired local output name differs from the account-side agent name:

```bash
cargo ai hatch local_weather \
  --agent weather_test \
  --owner-handle alice
```

## Account email workflows

Registration also enables account-backed email actions and mail preferences. Test delivery or inspect and change account-wide mail preference state with:

```bash
cargo ai mail test
cargo ai mail prefs
cargo ai mail prefs --disable-all
cargo ai mail prefs --enable-all
```

An agent's `email_me` action uses the active account session. Treat delivery as a real side effect: validate deterministic model and action behavior first, then test email last. See [Actions and child agents](./actions-and-child-agents.md) for the action contract.

## Packages use a separate hosted surface

Published packages are versioned project artifacts, not hosted agent-definition records. Manage them with `cargo ai packages`, including publication, public listings, install, update, rollback, visibility, and archive operations.

See [Build and package workflows](./packages.md) for package identity, publisher trust, permissions, persistent data, and installed runtime boundaries.

---

[Documentation hub](./README.md) · [Cargo AI README](../README.md)
