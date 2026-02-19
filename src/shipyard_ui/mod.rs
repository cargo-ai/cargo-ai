use eframe::egui::{self, Color32, RichText};
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

pub fn launch() -> Result<(), String> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Cargo AI Shipyard")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([900.0, 560.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Cargo AI Shipyard",
        native_options,
        Box::new(|cc| {
            configure_theme(&cc.egui_ctx);
            Ok(Box::new(ShipyardApp::new()))
        }),
    )
    .map_err(|e| e.to_string())
}

fn configure_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.visuals.window_fill = Color32::from_rgb(244, 245, 247);
    style.visuals.panel_fill = Color32::from_rgb(244, 245, 247);
    style.visuals.extreme_bg_color = Color32::from_rgb(246, 247, 248);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(219, 232, 255);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(234, 242, 255);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(238, 239, 241);
    ctx.set_style(style);
}

struct ShipyardApp {
    output_lines: Vec<String>,
    status: RunStatus,
    last_command: String,
    auto_started: bool,
    event_tx: Sender<TerminalEvent>,
    event_rx: Receiver<TerminalEvent>,
}

impl ShipyardApp {
    fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel();

        Self {
            output_lines: Vec::new(),
            status: RunStatus::Idle,
            last_command: String::new(),
            auto_started: false,
            event_tx,
            event_rx,
        }
    }

    fn run_default_command(&mut self) {
        if matches!(self.status, RunStatus::Running) {
            return;
        }

        let plan = default_command_plan();
        self.last_command = plan.display.clone();
        self.status = RunStatus::Running;
        self.output_lines
            .push(format!("$ {}", self.last_command.as_str()));

        spawn_command(plan, self.event_tx.clone());
    }

    fn flush_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                TerminalEvent::Output { stream, line } => {
                    let prefix = match stream {
                        StreamKind::Stdout => "stdout",
                        StreamKind::Stderr => "stderr",
                    };
                    self.output_lines.push(format!("{prefix} | {line}"));
                }
                TerminalEvent::Finished { success, code } => {
                    self.status = if success {
                        RunStatus::Succeeded(code)
                    } else {
                        RunStatus::Failed(code)
                    };
                }
                TerminalEvent::SpawnFailed(message) => {
                    self.status = RunStatus::SpawnError(message.clone());
                    self.output_lines.push(format!("stderr | {message}"));
                }
            }
        }
    }
}

impl eframe::App for ShipyardApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.flush_events();

        if !self.auto_started {
            self.auto_started = true;
            self.run_default_command();
        }

        if matches!(self.status, RunStatus::Running) {
            ctx.request_repaint_after(Duration::from_millis(40));
        }

        egui::TopBottomPanel::top("shipyard_title")
            .exact_height(52.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("●")
                            .size(16.0)
                            .color(Color32::from_rgb(251, 95, 87)),
                    );
                    ui.label(
                        RichText::new("●")
                            .size(16.0)
                            .color(Color32::from_rgb(252, 191, 63)),
                    );
                    ui.label(
                        RichText::new("●")
                            .size(16.0)
                            .color(Color32::from_rgb(41, 201, 67)),
                    );
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new("Shipyard")
                            .size(18.0)
                            .color(Color32::from_rgb(34, 38, 43))
                            .strong(),
                    );
                    ui.label(
                        RichText::new("Phase 1")
                            .size(13.0)
                            .color(Color32::from_rgb(106, 112, 120)),
                    );
                });
            });

        egui::TopBottomPanel::bottom("shipyard_execution")
            .default_height(250.0)
            .min_height(170.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Execution Feed")
                            .strong()
                            .size(15.0)
                            .color(Color32::from_rgb(36, 40, 45)),
                    );

                    let (status_label, status_color, status_code) = match &self.status {
                        RunStatus::Idle => ("idle", Color32::from_rgb(120, 128, 136), None),
                        RunStatus::Running => ("running", Color32::from_rgb(41, 201, 67), None),
                        RunStatus::Succeeded(code) => {
                            ("success", Color32::from_rgb(54, 156, 86), *code)
                        }
                        RunStatus::Failed(code) => {
                            ("failed", Color32::from_rgb(199, 57, 57), *code)
                        }
                        RunStatus::SpawnError(_) => {
                            ("spawn error", Color32::from_rgb(199, 57, 57), None)
                        }
                    };
                    ui.label(RichText::new(status_label).color(status_color).strong());
                    if let Some(code) = status_code {
                        ui.label(
                            RichText::new(format!("(exit {code})"))
                                .size(12.0)
                                .color(Color32::from_rgb(106, 112, 120)),
                        );
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can_run = !matches!(self.status, RunStatus::Running);
                        if ui
                            .add_enabled(can_run, egui::Button::new("Run `profile list`"))
                            .clicked()
                        {
                            self.run_default_command();
                        }
                    });
                });

                if !self.last_command.is_empty() {
                    ui.label(
                        RichText::new(format!("Command: {}", self.last_command))
                            .monospace()
                            .color(Color32::from_rgb(85, 92, 101)),
                    );
                }

                egui::Frame::new()
                    .fill(Color32::from_rgb(22, 24, 28))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .stick_to_bottom(true)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                if self.output_lines.is_empty() {
                                    ui.label(
                                        RichText::new("No command output yet.")
                                            .monospace()
                                            .color(Color32::from_rgb(147, 152, 160)),
                                    );
                                } else {
                                    for line in &self.output_lines {
                                        let text_color = if line.starts_with("stderr |") {
                                            Color32::from_rgb(255, 158, 158)
                                        } else {
                                            Color32::from_rgb(211, 218, 227)
                                        };
                                        ui.label(
                                            RichText::new(line)
                                                .monospace()
                                                .size(12.5)
                                                .color(text_color),
                                        );
                                    }
                                }
                            });
                    });

                if let RunStatus::SpawnError(message) = &self.status {
                    ui.label(
                        RichText::new(format!("Error: {message}"))
                            .color(Color32::from_rgb(199, 57, 57)),
                    );
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgb(248, 248, 249))
                .corner_radius(egui::CornerRadius::same(12))
                .inner_margin(egui::Margin::same(20))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(70.0);
                        ui.label(
                            RichText::new("Shipyard Workspace")
                                .size(26.0)
                                .color(Color32::from_rgb(43, 47, 54))
                                .strong(),
                        );
                        ui.label(
                            RichText::new("Primary interaction area (Phase 1 placeholder)")
                                .size(14.0)
                                .color(Color32::from_rgb(110, 116, 124)),
                        );
                        ui.add_space(14.0);
                    });
                    ui.allocate_space(ui.available_size());
                });
        });
    }
}

#[derive(Clone)]
struct CommandPlan {
    program: PathBuf,
    args: Vec<String>,
    display: String,
}

#[derive(Clone, Copy)]
enum StreamKind {
    Stdout,
    Stderr,
}

enum TerminalEvent {
    Output { stream: StreamKind, line: String },
    Finished { success: bool, code: Option<i32> },
    SpawnFailed(String),
}

enum RunStatus {
    Idle,
    Running,
    Succeeded(Option<i32>),
    Failed(Option<i32>),
    SpawnError(String),
}

fn default_command_plan() -> CommandPlan {
    if let Ok(current_exe) = std::env::current_exe() {
        let args = vec!["profile".to_string(), "list".to_string()];
        let display = format!("{} {}", current_exe.display(), args.join(" "));
        CommandPlan {
            program: current_exe,
            args,
            display,
        }
    } else {
        CommandPlan {
            program: PathBuf::from("cargo"),
            args: vec!["--version".to_string()],
            display: "cargo --version".to_string(),
        }
    }
}

fn spawn_command(plan: CommandPlan, tx: Sender<TerminalEvent>) {
    thread::spawn(move || {
        let mut child = match Command::new(&plan.program)
            .args(&plan.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                let _ = tx.send(TerminalEvent::SpawnFailed(format!(
                    "failed to start command '{}': {e}",
                    plan.display
                )));
                return;
            }
        };

        let mut readers = Vec::new();

        if let Some(stdout) = child.stdout.take() {
            readers.push(stream_lines(stdout, StreamKind::Stdout, tx.clone()));
        }

        if let Some(stderr) = child.stderr.take() {
            readers.push(stream_lines(stderr, StreamKind::Stderr, tx.clone()));
        }

        let status = match child.wait() {
            Ok(status) => status,
            Err(e) => {
                let _ = tx.send(TerminalEvent::SpawnFailed(format!(
                    "failed while waiting for command '{}': {e}",
                    plan.display
                )));
                return;
            }
        };

        for reader in readers {
            let _ = reader.join();
        }

        let _ = tx.send(TerminalEvent::Finished {
            success: status.success(),
            code: status.code(),
        });
    });
}

fn stream_lines<R>(
    reader: R,
    stream: StreamKind,
    tx: Sender<TerminalEvent>,
) -> thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let buffered = BufReader::new(reader);
        for line in buffered.lines() {
            match line {
                Ok(text) => {
                    let _ = tx.send(TerminalEvent::Output { stream, line: text });
                }
                Err(e) => {
                    let _ = tx.send(TerminalEvent::Output {
                        stream: StreamKind::Stderr,
                        line: format!("failed reading command output: {e}"),
                    });
                    break;
                }
            }
        }
    })
}
