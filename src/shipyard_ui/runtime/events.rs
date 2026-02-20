#[derive(Clone, Copy)]
pub enum StreamKind {
    Stdout,
    Stderr,
}

pub enum TerminalEvent {
    Output { stream: StreamKind, line: String },
    Finished { success: bool, code: Option<i32> },
    SpawnFailed(String),
}

pub enum RunStatus {
    Idle,
    Running,
    Succeeded(Option<i32>),
    Failed(Option<i32>),
    SpawnError(String),
}

impl RunStatus {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }
}
