//! Private native pipes and persistent host control, separate from package input.
use crate::role_runtime::{Bootstrap, Context};
use crate::role_session::{read_frame, Session};
#[cfg(cargo_ai_cli)]
use serde::Deserialize;
#[cfg(cargo_ai_cli)]
use serde_json::{json, Value};
use std::io::{self, BufReader};
#[cfg(cargo_ai_cli)]
use std::io::{BufRead, Write};
#[cfg(cargo_ai_cli)]
use std::sync::mpsc;

#[cfg(cargo_ai_cli)]
pub const CONTROL_ACK_DEADLINE_MS: u64 = 2_000;
#[cfg(cargo_ai_cli)]
pub const MAX_CONTROL_FRAMES: usize = 256;

#[cfg(cargo_ai_cli)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostStart {
    protocol: String,
    version: u32,
    #[serde(rename = "type")]
    kind: String,
    request: Value,
}
#[cfg(cargo_ai_cli)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostControl {
    protocol: String,
    version: u32,
    #[serde(rename = "type")]
    kind: String,
    binding_revision: String,
}
fn invalid() -> String {
    crate::role_runtime::failure("role.invalid_control")
}
#[cfg(cargo_ai_cli)]
pub fn host_start() -> Result<(String, Session), String> {
    let mut reader = BufReader::new(io::stdin());
    let value = read_frame(&mut reader)
        .map_err(|_| invalid())?
        .ok_or_else(invalid)?;
    let start: HostStart = serde_json::from_value(value).map_err(|_| invalid())?;
    if start.protocol != "cargo_ai_role_session" || start.version != 1 || start.kind != "start" {
        return Err(invalid());
    }
    let revision = start
        .request
        .pointer("/role_execution/binding_revision/revision")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    if !crate::role_contract::identifier(revision) {
        return Err(invalid());
    }
    let mut session = Session::new(revision.into());
    if let Some(id) = crate::commands::machine::operation_id() {
        session.invocation_id = id;
    }
    let control = session.clone();
    let acknowledgements = HostAcknowledgements::new(|bytes| {
        let mut output = io::stdout().lock();
        output.write_all(bytes)?;
        output.flush()
    });
    std::thread::spawn(move || {
        serve_host_controls(&mut reader, &control, &acknowledgements, || {
            crate::commands::machine_process::terminate_all().is_ok()
        });
    });
    let raw = serde_json::to_string(&start.request).map_err(|_| invalid())?;
    Ok((raw, session))
}

// A blocked stdout lock must not prevent the control reader from canceling the
// session. One bounded writer survives only until this short-lived process exits.
#[cfg(cargo_ai_cli)]
struct HostAcknowledgements {
    sender: mpsc::SyncSender<(Vec<u8>, mpsc::SyncSender<bool>)>,
}
#[cfg(cargo_ai_cli)]
impl HostAcknowledgements {
    fn new(mut write: impl FnMut(&[u8]) -> io::Result<()> + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<(Vec<u8>, mpsc::SyncSender<bool>)>(1);
        std::thread::spawn(move || {
            while let Ok((bytes, receipt)) = receiver.recv() {
                let written = write(&bytes).is_ok();
                let _ = receipt.try_send(written);
                if !written {
                    break;
                }
            }
        });
        Self { sender }
    }
    fn acknowledge(&self, reply: &Value) -> bool {
        let Ok(mut bytes) = serde_json::to_vec(reply) else {
            return false;
        };
        if bytes.len() + 1 > crate::role_session::MAX_FRAME_BYTES {
            return false;
        }
        bytes.push(b'\n');
        let (receipt, received) = mpsc::sync_channel(1);
        let enqueued_at = std::time::Instant::now();
        self.sender.try_send((bytes, receipt)).is_ok()
            && received
                .recv_timeout(
                    std::time::Duration::from_millis(CONTROL_ACK_DEADLINE_MS)
                        .saturating_sub(enqueued_at.elapsed()),
                )
                .unwrap_or(false)
    }
}

#[cfg(cargo_ai_cli)]
fn close_host_control(control: &Session, cleanup: &mut impl FnMut() -> bool, reason: &str) {
    control.close(reason);
    control.cancel();
    let _ = cleanup();
}

#[cfg(cargo_ai_cli)]
fn serve_host_controls(
    reader: &mut impl BufRead,
    control: &Session,
    acknowledgements: &HostAcknowledgements,
    mut cleanup: impl FnMut() -> bool,
) {
    // The existing per-frame bound also bounds total accepted wire input to
    // MAX_CONTROL_FRAMES * MAX_FRAME_BYTES, excluding the separately bounded start.
    for _ in 0..MAX_CONTROL_FRAMES {
        let frame = match read_frame(reader) {
            Ok(Some(value)) => value,
            Ok(None) => {
                close_host_control(control, &mut cleanup, "control_eof");
                return;
            }
            Err(_) => {
                close_host_control(control, &mut cleanup, "malformed_control");
                return;
            }
        };
        let input: HostControl = match serde_json::from_value(frame) {
            Ok(value) => value,
            Err(_) => {
                close_host_control(control, &mut cleanup, "malformed_control");
                return;
            }
        };
        if input.protocol != "cargo_ai_role_session"
            || input.version != 1
            || !matches!(input.kind.as_str(), "revoke" | "cancel")
        {
            close_host_control(control, &mut cleanup, "malformed_control");
            return;
        }
        let reply = match control.revoke(&input.binding_revision) {
            Ok(mut reply) => {
                if input.kind == "cancel" {
                    control.cancel();
                    reply["type"] = json!("canceled");
                    reply["owned_child_cleanup"] = json!(if cleanup() {
                        "requested"
                    } else {
                        "unconfirmed"
                    });
                }
                reply
            }
            Err(_) => {
                json!({"protocol":"cargo_ai_role_session","version":1,"type":"control_rejected","code":"role.revision_mismatch","invocation_id":control.invocation_id,"binding_revision":control.binding_revision})
            }
        };
        if !acknowledgements.acknowledge(&reply) {
            close_host_control(control, &mut cleanup, "control_output_lost");
            return;
        }
    }
    close_host_control(control, &mut cleanup, "control_limit");
}

pub fn native_child_start() -> Result<Context, String> {
    let mut reader = BufReader::new(io::stdin());
    let value = read_frame(&mut reader)
        .map_err(|_| invalid())?
        .ok_or_else(invalid)?;
    if value["protocol"] != "cargo_ai_native_role"
        || value["version"] != 1
        || value["type"] != "bootstrap"
    {
        return Err(invalid());
    }
    let bootstrap: Bootstrap =
        serde_json::from_value(value["bootstrap"].clone()).map_err(|_| invalid())?;
    if bootstrap.parent_permit_id.is_none() {
        return Err(invalid());
    }
    let session = Session::remote(
        bootstrap.invocation_id.clone(),
        bootstrap.binding_revision.clone(),
        Box::new(io::stdout()),
    );
    let context = Context::new(bootstrap, session.clone())?;
    std::thread::spawn(move || loop {
        match read_frame(&mut reader) {
            Ok(Some(value)) => {
                if session.accept_admission_reply(&value).is_err() {
                    session.close("malformed_control");
                    session.cancel();
                    break;
                }
            }
            Ok(None) => {
                session.close("control_eof");
                session.cancel();
                break;
            }
            Err(_) => {
                session.close("malformed_control");
                session.cancel();
                break;
            }
        }
    });
    Ok(context)
}

#[cfg(all(test, cargo_ai_cli))]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::time::{Duration, Instant};

    fn mismatched_control() -> Vec<u8> {
        let mut frame = serde_json::to_vec(&json!({"protocol":"cargo_ai_role_session","version":1,"type":"revoke","binding_revision":"other"})).unwrap();
        frame.push(b'\n');
        frame
    }

    #[tokio::test]
    async fn host_role_blocked_ack_cancels_without_reporting_admitted_work_completed() {
        let session = Session::new("current".into());
        let permit = session
            .admit(crate::role_session::Boundary {
                invocation_id: session.invocation_id.clone(),
                binding_revision: session.binding_revision.clone(),
                parent_permit_id: None,
                call_site: "root".into(),
                agent: "agent.json".into(),
                target_agent: None,
                kind: "provider".into(),
            })
            .await
            .unwrap();
        let (release, blocked) = mpsc::channel();
        let writer = HostAcknowledgements::new(move |_| {
            blocked
                .recv()
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "fixture released"))
        });
        let mut cleanup = 0;
        let started = Instant::now();
        serve_host_controls(
            &mut io::Cursor::new(mismatched_control()),
            &session,
            &writer,
            || {
                cleanup += 1;
                true
            },
        );
        assert!(started.elapsed() >= Duration::from_millis(CONTROL_ACK_DEADLINE_MS));
        assert_eq!(session.snapshot()["closed"], "control_output_lost");
        tokio::time::timeout(Duration::from_millis(50), session.canceled())
            .await
            .unwrap();
        assert_eq!(cleanup, 1);
        assert_eq!(
            session.snapshot()["outstanding"].as_object().unwrap().len(),
            1
        );
        drop(permit);
        assert_eq!(
            session.snapshot()["outcomes"][0]["state"],
            "completion_unknown"
        );
        release.send(()).unwrap();
    }

    #[test]
    fn host_role_broken_ack_and_control_budget_request_cleanup() {
        let session = Session::new("current".into());
        let broken = HostAcknowledgements::new(|_| {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "fixture"))
        });
        let mut cleanup = 0;
        serve_host_controls(
            &mut io::Cursor::new(mismatched_control()),
            &session,
            &broken,
            || {
                cleanup += 1;
                true
            },
        );
        assert_eq!(cleanup, 1);
        assert_eq!(session.snapshot()["closed"], "control_output_lost");

        let session = Session::new("current".into());
        let frame = mismatched_control();
        let mut input = io::Cursor::new(frame.repeat(MAX_CONTROL_FRAMES + 1));
        let receipts = Arc::new(AtomicUsize::new(0));
        let written = receipts.clone();
        let writer = HostAcknowledgements::new(move |_| {
            written.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        cleanup = 0;
        serve_host_controls(&mut input, &session, &writer, || {
            cleanup += 1;
            true
        });
        assert_eq!(receipts.load(Ordering::SeqCst), MAX_CONTROL_FRAMES);
        assert_eq!(input.position() as usize, MAX_CONTROL_FRAMES * frame.len());
        assert_eq!(session.snapshot()["closed"], "control_limit");
        assert_eq!(cleanup, 1);
    }

    #[test]
    fn host_role_full_ack_queue_fails_without_waiting_or_growing() {
        let (entered, active) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let mut first = true;
        let writer = HostAcknowledgements::new(move |_| {
            if first {
                first = false;
                entered.send(()).unwrap();
                blocked.recv().unwrap();
            }
            Ok(())
        });
        let (receipt, _received) = mpsc::sync_channel(1);
        writer
            .sender
            .try_send((b"{}\n".to_vec(), receipt.clone()))
            .unwrap();
        active.recv_timeout(Duration::from_secs(1)).unwrap();
        writer.sender.try_send((b"{}\n".to_vec(), receipt)).unwrap();
        let started = Instant::now();
        assert!(!writer.acknowledge(&json!({"type":"rejected"})));
        assert!(started.elapsed() < Duration::from_secs(1));
        release.send(()).unwrap();
    }
}
