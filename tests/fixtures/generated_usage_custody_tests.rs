#[cfg(test)]
mod generated_usage_custody_tests {
    use crate::usage_analytics::{self as cloud, contract::*, queue};
    use serde_json::{json, Value};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    fn id(n: u64) -> CanonicalUuid {
        CanonicalUuid::try_from(format!("00000000-0000-4000-8000-{n:012}")).unwrap()
    }
    fn read_request(stream: &mut std::net::TcpStream) -> Value {
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut bytes = Vec::new();
        let end = loop {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            bytes.push(byte[0]);
            assert!(bytes.len() < 16 * 1024);
            if bytes.ends_with(b"\r\n\r\n") {
                break bytes.len();
            }
        };
        let headers = std::str::from_utf8(&bytes).unwrap();
        let length: usize = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap())
            })
            .unwrap();
        assert!(length < 300 * 1024);
        bytes.resize(end + length, 0);
        stream.read_exact(&mut bytes[end..]).unwrap();
        serde_json::from_slice(&bytes[end..]).unwrap()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generated_usage_scheduler_recovers_exact_custody_after_response_loss() {
        assert_eq!(
            std::env::var("CARGO_AI_DISABLE_KEYCHAIN").as_deref(),
            Ok("1")
        );
        crate::persist_account_tokens_to_file("synthetic-usage-token", None).unwrap();
        let source = Source {
            account_binding: id(1),
            source_home_id: id(2),
            data_generation: Counter::try_from(1).unwrap(),
        };
        let provenance = Provenance {
            lifecycle_epoch: LifecycleEpoch::try_from(0).unwrap(),
            consent_interval: Counter::try_from(1).unwrap(),
        };
        let mut db = crate::usage_store::open_database(true).unwrap().unwrap();
        queue::initialize(&mut db).unwrap();
        queue::enable(&mut db, &source, &provenance, cloud::now_ms()).unwrap();
        db.execute(
            "INSERT INTO usage_meta(key,value) VALUES('usage_cloud_selected_v1',?1)",
            [json!({"source":source,"provenance":provenance}).to_string()],
        )
        .unwrap();
        let (_, mut guard) = crate::usage_log::UsageLogContext::from_runtime(None, 0, None)
            .unwrap()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        guard.finish_success();
        drop(guard);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/account", listener.local_addr().unwrap());
        let (first_seen, seen) = tokio::sync::oneshot::channel();
        let server = std::thread::spawn(move || {
            let accept = || {
                let until = std::time::Instant::now() + Duration::from_secs(15);
                loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                std::time::Instant::now() < until,
                                "bounded loopback request absent"
                            );
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("loopback accept: {error}"),
                    }
                }
            };
            let mut first = accept();
            let body = read_request(&mut first);
            assert_eq!(body["credentials"]["access_token"], "synthetic-usage-token");
            let frozen = body["usage_analytics"].clone();
            let request = Request::parse(&frozen.to_string()).unwrap();
            let Request::Sync {
                source,
                provenance,
                batch_id,
                ordered_membership_sha256,
                events,
                ..
            } = request
            else {
                panic!("initial request must submit frozen events")
            };
            assert!(events.len() >= 2);
            assert!(events
                .iter()
                .any(
                    |event| serde_json::to_value(event).unwrap()["timing"]["basis"]
                        == "monotonic_elapsed"
                ));
            let members = events
                .iter()
                .map(|event| event.member().unwrap())
                .collect::<Vec<_>>();
            drop(first); // An accepted request whose response is lost must not be blindly replayed.
            first_seen.send(()).unwrap();
            let mut second = accept();
            let recovery = read_request(&mut second);
            assert_eq!(recovery["usage_analytics"]["command"], "receipt");
            assert_eq!(recovery["usage_analytics"]["batch_id"], json!(batch_id));
            let manifest_sha256 = Sha256Hash::of(b"synthetic authenticated admission commitment");
            let admission = AdmissionCommitment {
                protocol_version: Version,
                source: source.clone(),
                provenance: provenance.clone(),
                batch_id: batch_id.clone(),
                ordered_membership_sha256: ordered_membership_sha256.clone(),
                manifest_sha256: manifest_sha256.clone(),
            };
            let receipt = Receipt {
                protocol_version: Version,
                source,
                provenance,
                batch_id,
                ordered_membership_sha256,
                manifest_sha256,
                custody: Custody::Retained,
                members,
            };
            let response = json!({"status":"success","type":"usage_analytics_succeeded","usage_analytics":{"custody":"retained","admission":admission,"receipt":receipt}}).to_string();
            write!(second,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
            frozen
        });
        cloud::TEST_ENDPOINT.with(|saved| saved.replace(Some(endpoint)));
        cloud::scheduler::scope(async {
            tokio::time::timeout(Duration::from_secs(10), seen)
                .await
                .unwrap()
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        })
        .await;
        let pending = cloud::local_status().unwrap();
        assert!(pending["pending"].as_u64().unwrap() >= 2);
        assert_eq!(pending["retained"], 0);
        let result = cloud::sync().await.unwrap();
        assert_eq!(result["recovered"], true);
        assert_eq!(cloud::local_status().unwrap()["pending"], 0);
        let frozen = server.join().unwrap();
        let stored: String = db
            .query_row("SELECT request_json FROM usage_cloud_batches", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&stored).unwrap(), frozen);
        assert!(
            db.query_row(
                "SELECT COUNT(*) FROM usage_cloud_batches WHERE receipt_json IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap()
                > 0
        );
        cloud::TEST_ENDPOINT.with(|saved| saved.replace(None));
    }
}
