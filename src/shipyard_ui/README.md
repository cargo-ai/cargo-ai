# Shipyard UI

Shipyard is an exploratory desktop UI for Cargo AI.
The command surface is `cargo ai shipyard`.
By default it is hidden/gated: launch with `cargo ai shipyard --experimental` or `CARGO_AI_ENABLE_SHIPYARD=1 cargo ai shipyard`.

## Folder Map
- `mod.rs`: Shipyard launch entrypoint.
- `app.rs`: Slint callback wiring, UI state sync, and command lifecycle control.
- `ui.rs`: Slint component definition for the window and view layout.
- `config.rs`: tunable constants for window/layout/timing/labels.
- `state.rs`: persisted Shipyard UI preferences.
- `runtime/commands.rs`: allowlisted command intents and command plans.
- `runtime/executor.rs`: process execution and output streaming.
- `runtime/events.rs`: runtime event and status enums.
- `assets/`: logo/image assets used by Shipyard UI.

## Tuning Knobs
Adjust these in `config.rs`:
- update timing/output controls:
  - `REPAINT_INTERVAL_MS`
  - `MAX_TERMINAL_LINES`
- execution view defaults:
  - `EXECUTION_VIEW_DEFAULT_VISIBLE`

## Runtime Behavior
- Shipyard uses allowlisted command intents (no free-form command entry).
- Startup intent runs `account status` and routes workspace to onboarding until authenticated.
- Onboarding path uses verbose commands:
  - `account register <email>`
  - `account confirm <code>`
  - `account status`
- Execution feed uses a bottom drawer toggle (collapsed/expanded) and persists visibility across launches.
- Title bar and workspace branding render from local Shipyard assets.
- Execution feed is read-only and shows:
  - command line
  - stdout/stderr lines
  - final status and exit code

## Security Guardrails
- No shell execution (`Command::new` direct process only).
- No stdin passthrough to child processes (`Stdio::null`).
- No interactive terminal input in UI (review-only output feed).
- Keep command intents allowlisted in `runtime/commands.rs`.
- Persisted UI state is stored at `$XDG_CONFIG_HOME/cargo-ai/shipyard_ui_state.json` (or platform-equivalent config directory).

## Extension Rules
- Keep `src/args.rs` and `src/main.rs` as thin dispatch-only entrypoints.
- Add new UI surfaces in `ui.rs`; keep orchestration and side effects in `app.rs`.
- Add new runtime actions as explicit intents in `runtime/commands.rs`.
- Keep command construction verbose and explicit (avoid short-form flag aliases by default).
