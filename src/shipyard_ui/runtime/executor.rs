use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;

use crate::shipyard_ui::runtime::commands::CommandPlan;
use crate::shipyard_ui::runtime::events::{StreamKind, TerminalEvent};

pub fn spawn_command(plan: CommandPlan, sender: Sender<TerminalEvent>) {
    thread::spawn(move || {
        // Security: run direct process execution only (no shell),
        // and keep stdin disconnected for read-only execution.
        let mut child = match Command::new(&plan.program)
            .args(&plan.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                let _ = sender.send(TerminalEvent::SpawnFailed(format!(
                    "failed to start command '{}': {error}",
                    plan.display
                )));
                return;
            }
        };

        let mut reader_handles = Vec::new();

        if let Some(stdout) = child.stdout.take() {
            reader_handles.push(stream_lines(stdout, StreamKind::Stdout, sender.clone()));
        }

        if let Some(stderr) = child.stderr.take() {
            reader_handles.push(stream_lines(stderr, StreamKind::Stderr, sender.clone()));
        }

        let status = match child.wait() {
            Ok(status) => status,
            Err(error) => {
                let _ = sender.send(TerminalEvent::SpawnFailed(format!(
                    "failed while waiting for command '{}': {error}",
                    plan.display
                )));
                return;
            }
        };

        for handle in reader_handles {
            let _ = handle.join();
        }

        let _ = sender.send(TerminalEvent::Finished {
            success: status.success(),
            code: status.code(),
        });
    });
}

fn stream_lines<R>(
    reader: R,
    stream: StreamKind,
    sender: Sender<TerminalEvent>,
) -> thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let buffered = BufReader::new(reader);
        for line in buffered.lines() {
            match line {
                Ok(text) => {
                    let _ = sender.send(TerminalEvent::Output { stream, line: text });
                }
                Err(error) => {
                    let _ = sender.send(TerminalEvent::Output {
                        stream: StreamKind::Stderr,
                        line: format!("failed reading command output: {error}"),
                    });
                    break;
                }
            }
        }
    })
}
