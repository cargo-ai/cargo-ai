pub const WINDOW_INITIAL_HEIGHT: f32 = 760.0;

pub const REPAINT_INTERVAL_MS: u64 = 40;
pub const MAX_TERMINAL_LINES: usize = 1200;

pub const EXECUTION_PANEL_DEFAULT_RATIO: f32 = 0.33;
pub const EXECUTION_PANEL_MIN_RATIO: f32 = 0.26;
pub const EXECUTION_PANEL_MAX_RATIO: f32 = 0.55;

pub const ACCOUNT_STATUS_VERBOSE_ARGS: &[&str] = &["account", "status"];
pub const ACCOUNT_REGISTER_VERBOSE_PREFIX_ARGS: &[&str] = &["account", "register"];
pub const ACCOUNT_CONFIRM_VERBOSE_PREFIX_ARGS: &[&str] = &["account", "confirm"];

pub fn clamp_execution_panel_ratio(ratio: f32) -> f32 {
    ratio
        .max(EXECUTION_PANEL_MIN_RATIO)
        .min(EXECUTION_PANEL_MAX_RATIO)
}
