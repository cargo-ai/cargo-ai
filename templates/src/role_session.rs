//! Invocation-wide admission and bounded native control frames.
//!
//! A grant is admitted under the same mutex that closes the gate. A revocation
//! acknowledgement therefore describes every effect that can still settle;
//! receivers must not treat acknowledgement as rollback of admitted effects.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
const MAX_OUTSTANDING: usize = 256;
const MAX_RECORDS: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Boundary {
    pub invocation_id: String,
    pub binding_revision: String,
    pub parent_permit_id: Option<String>,
    pub call_site: String,
    pub agent: String,
    pub target_agent: Option<String>,
    pub kind: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    pub boundary: Boundary,
    pub permit_id: String,
    pub state: String,
    pub cause: Option<String>,
}
#[derive(Default)]
struct Gate {
    closed: Option<String>,
    outstanding: BTreeMap<String, Boundary>,
    outcomes: Vec<Outcome>,
    omitted: usize,
}
#[derive(Clone)]
pub struct Session {
    pub invocation_id: String,
    pub binding_revision: String,
    gate: Arc<Mutex<Gate>>,
    remote: Option<Arc<Remote>>,
    canceled: Arc<tokio::sync::Notify>,
    cancel_flag: Arc<std::sync::atomic::AtomicBool>,
}
struct Remote {
    writer: Mutex<Box<dyn Write + Send>>,
    waiting: Mutex<BTreeMap<String, tokio::sync::oneshot::Sender<Result<String, String>>>>,
}
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("invocation_id", &self.invocation_id)
            .finish_non_exhaustive()
    }
}
pub struct Permit {
    session: Session,
    id: String,
    boundary: Boundary,
    settled: bool,
}
impl Session {
    #[cfg(any(cargo_ai_cli, test))]
    pub fn new(binding_revision: String) -> Self {
        Self {
            invocation_id: uuid::Uuid::new_v4().to_string(),
            binding_revision,
            gate: Arc::new(Mutex::new(Gate::default())),
            remote: None,
            canceled: Arc::new(tokio::sync::Notify::new()),
            cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
    pub fn remote(
        invocation_id: String,
        binding_revision: String,
        writer: Box<dyn Write + Send>,
    ) -> Self {
        Self {
            invocation_id,
            binding_revision,
            gate: Arc::new(Mutex::new(Gate::default())),
            canceled: Arc::new(tokio::sync::Notify::new()),
            cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            remote: Some(Arc::new(Remote {
                writer: Mutex::new(writer),
                waiting: Mutex::new(BTreeMap::new()),
            })),
        }
    }
    pub fn cancel(&self) {
        self.close("canceled");
        self.cancel_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.canceled.notify_waiters();
    }
    pub async fn canceled(&self) {
        loop {
            let notified = self.canceled.notified();
            if self.cancel_flag.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
    pub fn is_closed(&self) -> bool {
        self.gate
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed
            .is_some()
    }
    pub fn close(&self, reason: &str) -> Value {
        let mut gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        let duplicate = gate.closed.is_some();
        gate.closed.get_or_insert_with(|| reason.to_owned());
        if let Some(remote) = &self.remote {
            remote
                .waiting
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
        }
        json!({"protocol":"cargo_ai_role_session","version":PROTOCOL_VERSION,"type":"revoked",
            "invocation_id":self.invocation_id,"binding_revision":self.binding_revision,
            "duplicate":duplicate,"reason":gate.closed,"outstanding":gate.outstanding,
            "effects":"admitted_work_may_settle"})
    }
    #[cfg(any(cargo_ai_cli, test))]
    pub fn revoke(&self, revision: &str) -> Result<Value, String> {
        if revision != self.binding_revision {
            return Err("role.revision_mismatch".into());
        }
        Ok(self.close("revoked"))
    }
    pub fn record_denial(&self, boundary: Boundary, code: &str) {
        let mut gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        if gate.outcomes.len() < MAX_RECORDS {
            gate.outcomes.push(Outcome {
                boundary: boundary.clone(),
                permit_id: String::new(),
                state: "not_dispatched".into(),
                cause: Some(code.into()),
            });
        } else {
            gate.omitted += 1;
        }
        drop(gate);
        if self.remote.is_some() {
            let _=self.send(&json!({"protocol":"cargo_ai_native_role","version":1,"type":"denied","boundary":boundary,"code":code}));
        }
    }
    pub fn snapshot(&self) -> Value {
        let gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        json!({"invocation_id":self.invocation_id,"binding_revision":self.binding_revision,
            "closed":gate.closed,"outstanding":gate.outstanding,"outcomes":gate.outcomes,
            "omitted_outcomes":gate.omitted,"complete":gate.outstanding.is_empty()})
    }
    pub async fn admit(&self, boundary: Boundary) -> Result<Permit, String> {
        if boundary.invocation_id != self.invocation_id
            || boundary.binding_revision != self.binding_revision
            || boundary.call_site.is_empty()
            || boundary.call_site.len() > 256
            || boundary.agent.len() > 1024
            || boundary
                .parent_permit_id
                .as_ref()
                .is_some_and(|id| id.is_empty() || id.len() > 128)
            || boundary
                .target_agent
                .as_ref()
                .is_some_and(|target| target.is_empty() || target.len() > 1024)
            || !matches!(boundary.kind.as_str(), "provider" | "native_child" | "tool")
        {
            return Err("role.invalid_admission".into());
        }
        if self.is_closed() {
            return Err("role.revoked".into());
        }
        let id = if let Some(remote) = &self.remote {
            let request_id = uuid::Uuid::new_v4().to_string();
            let (sender, receiver) = tokio::sync::oneshot::channel();
            {
                let mut waiting = remote.waiting.lock().unwrap_or_else(|e| e.into_inner());
                if waiting.len() >= MAX_OUTSTANDING {
                    return Err("role.admission_limit".into());
                }
                waiting.insert(request_id.clone(), sender);
            }
            if self.send(&json!({"protocol":"cargo_ai_native_role","version":1,"type":"admit","request_id":request_id,"boundary":boundary})).is_err() {
                self.close("control_lost"); return Err("role.control_lost".into());
            }
            match tokio::time::timeout(std::time::Duration::from_secs(30), receiver).await {
                Ok(Ok(result)) => result?,
                _ => {
                    self.close("control_lost");
                    return Err("role.control_lost".into());
                }
            }
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        // For a remote session the parent has already counted this permit as
        // admitted. Local closure may refuse to use it, but must still release it.
        let mut gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        if gate.closed.is_some() || gate.outstanding.len() >= MAX_OUTSTANDING {
            drop(gate);
            if self.remote.is_some() {
                let _=self.send(&json!({"protocol":"cargo_ai_native_role","version":1,"type":"settled","permit_id":id,"state":"not_dispatched","cause":null}));
            }
            return Err("role.revoked".into());
        }
        gate.outstanding.insert(id.clone(), boundary.clone());
        drop(gate);
        Ok(Permit {
            session: self.clone(),
            id,
            boundary,
            settled: false,
        })
    }
    fn send(&self, value: &Value) -> io::Result<()> {
        let remote = self
            .remote
            .as_ref()
            .ok_or_else(|| io::Error::other("Not a remote session"))?;
        write_frame(
            &mut *remote.writer.lock().unwrap_or_else(|e| e.into_inner()),
            value,
        )
    }
    pub fn accept_admission_reply(&self, value: &Value) -> Result<(), String> {
        let remote = self.remote.as_ref().ok_or("role.invalid_control")?;
        if value["protocol"] != "cargo_ai_native_role"
            || value["version"] != 1
            || value["type"] != "admission"
        {
            return Err("role.invalid_control".into());
        }
        let request = value["request_id"].as_str().ok_or("role.invalid_control")?;
        let sender = remote
            .waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(request)
            .ok_or("role.invalid_control")?;
        let result = if value["allowed"] == true {
            Ok(value["permit_id"]
                .as_str()
                .filter(|v| v.len() <= 128)
                .ok_or("role.invalid_control")?
                .to_owned())
        } else {
            Err("role.revoked".to_owned())
        };
        let _ = sender.send(result);
        Ok(())
    }
}
impl Permit {
    pub fn boundary(&self) -> &Boundary {
        &self.boundary
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn finish(mut self, state: &str, cause: Option<&str>) {
        self.settle(state, cause);
    }
    fn settle(&mut self, state: &str, cause: Option<&str>) {
        if self.settled {
            return;
        }
        self.settled = true;
        let outcome = Outcome {
            boundary: self.boundary.clone(),
            permit_id: self.id.clone(),
            state: state.to_owned(),
            cause: cause.map(str::to_owned),
        };
        {
            let mut gate = self.session.gate.lock().unwrap_or_else(|e| e.into_inner());
            gate.outstanding.remove(&self.id);
            if gate.outcomes.len() < MAX_RECORDS {
                gate.outcomes.push(outcome);
            } else {
                gate.omitted += 1;
            }
        }
        if self.session.remote.is_some() {
            if self.session.send(&json!({"protocol":"cargo_ai_native_role","version":1,"type":"settled","permit_id":self.id,"state":state,"cause":cause})).is_err() { self.session.close("control_lost"); }
        }
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        self.settle("completion_unknown", Some("interrupted"));
    }
}
/// Reads one complete frame without ever allocating an unbounded line. A final
/// unterminated frame is a broken control channel, never a valid request.
pub fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Incomplete control frame",
                ))
            };
        }
        let count = available
            .iter()
            .position(|b| *b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(available.len());
        if bytes.len() + count > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Control frame exceeds bound",
            ));
        }
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count);
        if bytes.last() == Some(&b'\n') {
            break;
        }
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Malformed control frame"))
}
pub fn write_frame(writer: &mut (impl Write + ?Sized), value: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() + 1 > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Control frame exceeds bound",
        ));
    }
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn boundary(session: &Session, n: usize) -> Boundary {
        Boundary {
            invocation_id: session.invocation_id.clone(),
            binding_revision: session.binding_revision.clone(),
            parent_permit_id: None,
            call_site: format!("site-{n}"),
            agent: "agent.json".into(),
            target_agent: None,
            kind: "provider".into(),
        }
    }
    #[tokio::test]
    async fn acknowledgement_closes_one_shared_gate_for_every_lane() {
        let session = Session::new("revision-a".into());
        let permit = session.admit(boundary(&session, 0)).await.unwrap();
        let ack = session.revoke("revision-a").unwrap();
        assert_eq!(ack["outstanding"].as_object().unwrap().len(), 1);
        let mut joins = Vec::new();
        for n in 1..20 {
            let s = session.clone();
            joins.push(tokio::spawn(async move {
                s.admit(boundary(&s, n)).await.is_err()
            }));
        }
        for join in joins {
            assert!(join.await.unwrap());
        }
        permit.finish("completed", None);
        assert!(session.snapshot()["outstanding"]
            .as_object()
            .unwrap()
            .is_empty());
        assert_eq!(session.revoke("revision-a").unwrap()["duplicate"], true);
        assert!(session.revoke("revision-b").is_err());
    }
    #[tokio::test]
    async fn dropped_admitted_work_is_unknown_not_success() {
        let s = Session::new("a".into());
        drop(s.admit(boundary(&s, 0)).await.unwrap());
        assert_eq!(s.snapshot()["outcomes"][0]["state"], "completion_unknown");
    }
    #[test]
    fn framing_rejects_partial_oversized_and_malformed_without_eof_bootstrap() {
        assert!(read_frame(&mut io::Cursor::new(b"{}")).is_err());
        assert!(read_frame(&mut io::Cursor::new(vec![b'a'; MAX_FRAME_BYTES + 1])).is_err());
        assert!(read_frame(&mut io::Cursor::new(b"bad\n")).is_err());
        let mut stream = io::Cursor::new(b"{}\n{\"type\":\"revoke\"}\n");
        assert_eq!(read_frame(&mut stream).unwrap(), Some(json!({})));
        assert_eq!(read_frame(&mut stream).unwrap().unwrap()["type"], "revoke");
    }
}
