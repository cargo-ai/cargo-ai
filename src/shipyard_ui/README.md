# Shipyard UI

Shipyard is an exploratory desktop UI for Cargo AI.
The command surface is `cargo ai shipyard`.

## Folder Map
- `mod.rs`: UI launch entrypoint only.
- `app.rs`: top-level app state, event handling, update loop.
- `config.rs`: tunable constants for window/layout/timing/labels.
- `theme.rs`: UI visual theme configuration.
- `layout.rs`: panel composition and high-level view assembly.
- `runtime/commands.rs`: allowlisted command intents and command plans.
- `runtime/executor.rs`: process execution and output streaming.
- `runtime/events.rs`: runtime event and status enums.
- `widgets/title_bar.rs`: top bar rendering.
- `widgets/workspace.rs`: primary workspace rendering.
- `widgets/execution_feed.rs`: read-only command feed rendering.

## Tuning Knobs
Adjust these in `config.rs`:
- window dimensions:
  - `WINDOW_INITIAL_WIDTH`
  - `WINDOW_INITIAL_HEIGHT`
  - `WINDOW_MIN_WIDTH`
  - `WINDOW_MIN_HEIGHT`
- execution panel sizing:
  - `EXECUTION_PANEL_DEFAULT_HEIGHT`
  - `EXECUTION_PANEL_MIN_HEIGHT`
  - `EXECUTION_PANEL_TARGET_RATIO`
- update timing:
  - `REPAINT_INTERVAL_MS`
- terminal look:
  - `TERMINAL_FONT_SIZE`
  - `TERMINAL_CORNER_RADIUS`

## Runtime Behavior
- Shipyard uses allowlisted command intents (no free-form command entry).
- Current default intent runs the verbose command `profile list`.
- Execution feed is read-only and shows:
  - command line
  - stdout/stderr lines
  - final status and exit code

## Security Guardrails
- No shell execution (`Command::new` direct process only).
- No stdin passthrough to child processes (`Stdio::null`).
- No interactive terminal input in UI (review-only output feed).
- Keep command intents allowlisted in `runtime/commands.rs`.

## Extension Rules
- Keep `src/args.rs` and `src/main.rs` as thin dispatch-only entrypoints.
- Add new UI screens/components under `widgets/` and wire them in `layout.rs`.
- Add new runtime actions as explicit intents in `runtime/commands.rs`.
- Keep command construction verbose and explicit (avoid short-form flag aliases by default).
