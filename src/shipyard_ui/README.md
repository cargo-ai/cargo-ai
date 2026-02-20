# Shipyard UI

Shipyard is an exploratory desktop UI for Cargo AI.
The command surface is `cargo ai shipyard`.

## Folder Map
- `mod.rs`: UI launch entrypoint only.
- `app.rs`: top-level app state, event handling, update loop.
- `config.rs`: tunable constants for window/layout/timing/labels.
- `theme.rs`: UI visual theme configuration.
- `layout.rs`: panel composition and high-level view assembly.
- `assets.rs`: loading embedded Shipyard image assets into egui textures.
- `assets/`: logo/image assets used by Shipyard UI.
- `runtime/commands.rs`: allowlisted command intents and command plans.
- `runtime/executor.rs`: process execution and output streaming.
- `runtime/events.rs`: runtime event and status enums.
- `widgets/title_bar.rs`: top bar rendering.
- `widgets/workspace.rs`: primary workspace rendering.
- `widgets/account_onboarding.rs`: account-first setup UI (register/confirm/status path).
- `widgets/execution_feed.rs`: read-only command feed rendering.

## Tuning Knobs
Adjust these in `config.rs`:
- window dimensions:
  - `WINDOW_INITIAL_WIDTH`
  - `WINDOW_INITIAL_HEIGHT`
  - `WINDOW_MIN_WIDTH`
  - `WINDOW_MIN_HEIGHT`
- execution panel sizing:
  - `EXECUTION_PANEL_DEFAULT_RATIO`
  - `EXECUTION_PANEL_MIN_HEIGHT`
  - `EXECUTION_PANEL_MAX_RATIO`
- update timing:
  - `REPAINT_INTERVAL_MS`
  - `LOW_DPI_PPP_THRESHOLD`
  - `LOW_DPI_ZOOM_FACTOR`
- terminal look:
  - `TERMINAL_FONT_SIZE`
  - `TERMINAL_CORNER_RADIUS`
- onboarding typography:
  - `ONBOARDING_TITLE_FONT_SIZE`
  - `ONBOARDING_STATUS_FONT_SIZE`
  - `ONBOARDING_SUBTITLE_FONT_SIZE`
  - `ONBOARDING_SECTION_FONT_SIZE`
  - `ONBOARDING_INPUT_FONT_SIZE`
  - `ONBOARDING_BUTTON_FONT_SIZE`

## Runtime Behavior
- Shipyard uses allowlisted command intents (no free-form command entry).
- Startup intent runs `account status` and routes workspace to onboarding until authenticated.
- Onboarding path uses verbose commands:
  - `account register <email>`
  - `account confirm <code>`
  - `account status`
- First-launch panel split is ratio-based; user resize is persisted for future launches.
- Title bar and workspace branding render from local Shipyard assets.
- Shipyard applies low-DPI zoom compensation to improve readability on lower-density displays.
- Execution feed is read-only and shows:
  - command line
  - stdout/stderr lines
  - final status and exit code

## Security Guardrails
- No shell execution (`Command::new` direct process only).
- No stdin passthrough to child processes (`Stdio::null`).
- No interactive terminal input in UI (review-only output feed).
- Keep command intents allowlisted in `runtime/commands.rs`.
- Persisted panel height state is stored at `$XDG_CONFIG_HOME/cargo-ai/shipyard_ui_state.json` (or platform-equivalent config directory).

## Extension Rules
- Keep `src/args.rs` and `src/main.rs` as thin dispatch-only entrypoints.
- Add new UI screens/components under `widgets/` and wire them in `layout.rs`.
- Add new runtime actions as explicit intents in `runtime/commands.rs`.
- Keep command construction verbose and explicit (avoid short-form flag aliases by default).
