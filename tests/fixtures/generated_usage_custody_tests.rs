#[cfg(test)]
mod generated_usage_custody_tests {
    use crate::usage_analytics::{self as cloud, contract::*, elastic, queue};
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};

    fn id(n: u64) -> CanonicalUuid {
        CanonicalUuid::try_from(format!("00000000-0000-4000-8000-{n:012}")).unwrap()
    }
    fn read_request(stream: &mut TcpStream) -> (String, Value) {
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
        let length: usize = std::str::from_utf8(&bytes)
            .unwrap()
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap())
            })
            .unwrap();
        assert!(length <= elastic::MAX_ACCOUNT_BYTES);
        bytes.resize(end + length, 0);
        stream.read_exact(&mut bytes[end..]).unwrap();
        let text = String::from_utf8(bytes[end..].to_vec()).unwrap();
        let body: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(body["credentials"]["access_token"], "synthetic-usage-token");
        assert_eq!(body["action"], "usage_analytics");
        assert_eq!(body["usage_analytics"]["protocol_version"], 2);
        (text, body)
    }
    fn accept(listener: &TcpListener) -> TcpStream {
        let until = Instant::now() + Duration::from_secs(15);
        loop {
            match listener.accept() {
                Ok((socket, _)) => return socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < until, "bounded loopback request absent");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("loopback accept: {error}"),
            }
        }
    }
    fn respond(stream: &mut TcpStream, body: Value) {
        let response =
            json!({"status":"success","type":"usage_analytics_succeeded","usage_analytics":body})
                .to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
    }
    fn status(source: &Source, provenance: &Provenance) -> Value {
        json!({"protocol_version":2,"capability":{"enabled":true,"supported_protocol_versions":[2]},"account_binding":source.account_binding,"data_generation":source.data_generation,"lifecycle_epoch":provenance.lifecycle_epoch,"environments":[{"source_home_id":source.source_home_id,"consent_interval":provenance.consent_interval}],"policy":{"retention_days":7,"retained_event_target":1_000_000,"max_active_environments":3},"processing_status":"not_observed"})
    }
    fn upload(body: &Value) -> (elastic::Request, Vec<EventSubmission>) {
        let request = body["usage_analytics"]
            .to_string()
            .parse::<elastic::Request>()
            .unwrap();
        let elastic::Request::Upload {
            source,
            batch,
            ordered_membership_sha256,
            ..
        } = &request
        else {
            panic!("expected explicit V2 upload")
        };
        let events = elastic::decode_batch(batch).unwrap();
        assert!(
            events
                .iter()
                .all(|event| !event.original_text.contains("local only"))
        );
        let members = events
            .iter()
            .map(|event| event.member().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            elastic::membership_sha256(source, &members).unwrap(),
            *ordered_membership_sha256
        );
        assert!(events.iter().any(|event| event.timing.is_some()) || events.len() == 1);
        let hours = events
            .iter()
            .map(|event| {
                serde_json::from_str::<Value>(&event.original_text).unwrap()["occurred_at_ms"]
                    .as_i64()
                    .unwrap()
                    / 3_600_000
            })
            .collect::<Vec<_>>();
        assert!(hours.windows(2).all(|pair| pair[0] >= pair[1]));
        (request, events)
    }
    fn acknowledgment(request: &elastic::Request) -> Value {
        let elastic::Request::Upload {
            source,
            provenance,
            batch_id,
            ordered_membership_sha256,
            batch,
            ..
        } = request
        else {
            unreachable!()
        };
        let ack = elastic::UploadAck {
            protocol_version: elastic::Version,
            source: source.clone(),
            provenance: provenance.clone(),
            batch_id: batch_id.clone(),
            ordered_membership_sha256: ordered_membership_sha256.clone(),
            encoded_sha256: batch.encoded_sha256.clone(),
            acknowledged_events: batch.event_count,
            durable_acknowledged: true,
            processing_status: elastic::ProcessingStatus::NotObserved,
        };
        ack.verify(request).unwrap();
        json!(ack)
    }
    fn raw(db: &rusqlite::Connection) -> Vec<(String, String)> {
        db.prepare("SELECT event_id,record_json FROM usage_event_records ORDER BY sequence")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }
    async fn record_run() {
        let log = crate::usage_store::directory().join("generated-usage.ndjson");
        let (_, mut guard) = crate::usage_log::UsageLogContext::from_runtime(log.to_str(), 0, None)
            .unwrap()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        guard.finish_success();
    }
    fn late_event(db: &rusqlite::Connection, n: u64, offset_ms: i64) -> String {
        let event_id = id(n).as_str().to_owned();
        let stamp = time::OffsetDateTime::from_unix_timestamp_nanos(
            i128::from(cloud::now_ms() - offset_ms) * 1_000_000,
        )
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
        crate::usage_store::insert_event(db, &json!({"schema_version":1,"event_id":event_id,"root_run_id":id(10),"agent_run_id":id(11),"event_type":"provider_request_completed","timestamp":stamp,"usage":{"input_tokens":3,"output_tokens":1},"status":"success","private_extension":{"preserved":"local only"}})).unwrap();
        event_id
    }
    fn assert_no_request(listener: &TcpListener) {
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "local maintenance must not initiate a cloud request"
        );
    }
    fn generated_timer_proof() {
        for (name, started, exited, collected) in [
            (
                "usage_analytics::scheduler::tests::real_timer_wakes_during_active_work_and_stops_at_scope_end",
                "scheduler-timer-started",
                "scheduler-timer-exited",
                "scheduler-timer-collected",
            ),
            (
                "usage_analytics::scheduler::tests::real_process_cancellation_stops_the_production_scoped_timer",
                "scheduler-cancel-started",
                "scheduler-cancel-exited",
                "scheduler-cancel-collected",
            ),
        ] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", name, "--test-threads=1", "--nocapture"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            eprintln!("generated-usage-custody stage={started}");
            let mut stdout = child.stdout.take().unwrap();
            let mut stderr = child.stderr.take().unwrap();
            let stdout_reader = std::thread::spawn(move || {
                let mut bytes = Vec::new();
                stdout.read_to_end(&mut bytes).unwrap();
                bytes
            });
            let stderr_reader = std::thread::spawn(move || {
                let mut bytes = Vec::new();
                stderr.read_to_end(&mut bytes).unwrap();
                bytes
            });
            let status = child.wait().unwrap();
            eprintln!("generated-usage-custody stage={exited}");
            let output = std::process::Output {
                status,
                stdout: stdout_reader.join().unwrap(),
                stderr: stderr_reader.join().unwrap(),
            };
            eprintln!("generated-usage-custody stage={collected}");
            assert!(
                output.status.success(),
                "generated scheduler proof failed: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"));
        }
    }
    struct Endpoint(Option<String>);
    impl Drop for Endpoint {
        fn drop(&mut self) {
            cloud::TEST_ENDPOINT.with(|saved| saved.replace(self.0.take()));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generated_usage_explicit_upload_preserves_independent_local_history() {
        eprintln!("generated-usage-custody stage=entered");
        assert_eq!(
            std::env::var("CARGO_AI_DISABLE_KEYCHAIN").as_deref(),
            Ok("1")
        );
        assert!(crate::usage_store::tracking_enabled().unwrap());
        crate::usage_store::retention::configure(crate::usage_store::retention::Policy {
            schema_version: 2,
            days: 7,
            enabled: true,
            local_detail_loss_acknowledged: true,
            unselected_loss_acknowledged: true,
        })
        .unwrap();
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
            "INSERT INTO usage_meta(key,value) VALUES('usage_cloud_selected_v2',?1)",
            [
                json!({"source":source,"provenance":provenance,"cloud_retention_days":7})
                    .to_string(),
            ],
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let observer = listener.try_clone().unwrap();
        let endpoint = format!("http://{}/account", listener.local_addr().unwrap());
        let _endpoint = Endpoint(cloud::TEST_ENDPOINT.with(|saved| saved.replace(Some(endpoint))));

        cloud::scheduler::scope(record_run()).await;
        eprintln!("generated-usage-custody stage=local-recording-complete");
        generated_timer_proof();
        eprintln!("generated-usage-custody stage=scheduler-proof-complete");
        assert_no_request(&observer);
        let initial_raw = raw(&db);
        assert!(initial_raw.len() >= 2);
        assert_eq!(cloud::local_status().unwrap()["uploaded"], 0);
        assert!(
            cloud::local_status().unwrap()["unscanned"]
                .as_u64()
                .unwrap()
                >= 2
        );
        let server_source = source.clone();
        let server_provenance = provenance.clone();
        let server = std::thread::spawn(move || {
            let mut policy = accept(&listener);
            let (_, body) = read_request(&mut policy);
            assert_eq!(body["usage_analytics"]["command"], "status");
            respond(&mut policy, status(&server_source, &server_provenance));
            drop(policy);
            let mut first = accept(&listener);
            let (first_text, first_body) = read_request(&mut first);
            let (_, first_events) = upload(&first_body);
            let first_ids = first_events
                .iter()
                .map(|event| event.member().unwrap().event_id)
                .collect::<BTreeSet<_>>();
            drop(first); // The durable outcome is unknown after an accepted request loses its response.
            let mut policy = accept(&listener);
            let (_, body) = read_request(&mut policy);
            assert_eq!(body["usage_analytics"]["command"], "status");
            respond(&mut policy, status(&server_source, &server_provenance));
            drop(policy);
            let mut retry = accept(&listener);
            let (retry_text, retry_body) = read_request(&mut retry);
            assert_eq!(
                retry_text, first_text,
                "lost response must preserve exact authenticated compressed bytes"
            );
            let (retry_request, _) = upload(&retry_body);
            respond(&mut retry, acknowledgment(&retry_request));
            drop(retry);
            let mut next = accept(&listener);
            let (_, next_body) = read_request(&mut next);
            let (next_request, next_events) = upload(&next_body);
            let next_ids = next_events
                .iter()
                .map(|event| event.member().unwrap().event_id)
                .collect::<BTreeSet<_>>();
            assert!(first_ids.is_disjoint(&next_ids));
            assert!(
                next_ids.contains(id(100).as_str()),
                "late earlier-hour event must remain independently eligible"
            );
            respond(&mut next, acknowledgment(&next_request));
            (first_ids.len(), next_ids.len())
        });
        eprintln!("generated-usage-custody stage=first-sync-started");
        assert!(cloud::sync().await.is_err());
        eprintln!("generated-usage-custody stage=first-sync-complete");
        assert_eq!(raw(&db), initial_raw);
        assert_eq!(cloud::local_status().unwrap()["pending"], initial_raw.len());
        assert_eq!(cloud::local_status().unwrap()["unknown_batches"], 1);
        let frozen: String = db
            .query_row(
                "SELECT request_json FROM usage_cloud_upload_batches",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(frozen.parse::<elastic::Request>().is_ok());
        cloud::scheduler::scope(record_run()).await;
        late_event(&db, 100, 3_600_000);
        let before_ack = raw(&db);
        assert!(before_ack.len() > initial_raw.len());
        eprintln!("generated-usage-custody stage=retry-sync-started");
        let result = cloud::sync().await.unwrap();
        eprintln!("generated-usage-custody stage=retry-sync-complete");
        let (first_count, next_count) = server.join().unwrap();
        eprintln!("generated-usage-custody stage=server-joined");
        assert_eq!(result["acknowledged_events"], first_count + next_count);
        assert_eq!(result["processing_status"], "not_observed");
        assert_eq!(cloud::local_status().unwrap()["uploaded"], before_ack.len());
        assert_eq!(cloud::local_status().unwrap()["pending"], 0);
        assert_eq!(cloud::local_status().unwrap()["unknown_batches"], 0);
        assert_eq!(
            raw(&db),
            before_ack,
            "an upload ACK must preserve richer in-window local history"
        );
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM usage_cloud_upload_batches",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_no_request(&observer);

        late_event(&db, 101, 0);
        queue::capture(&mut db, &source, cloud::project).unwrap();
        assert_eq!(cloud::local_status().unwrap()["pending"], 1);
        let expired =
            crate::usage_store::retention::apply(cloud::now_ms() + 8 * 86_400_000).unwrap();
        assert_eq!(expired["maintenance_pending"], false);
        assert_eq!(expired["deleted_records"], before_ack.len() + 1);
        assert!(
            expired["expired_without_acknowledgment_records"]
                .as_u64()
                .unwrap()
                >= 1
        );
        assert!(raw(&db).is_empty());
        assert!(
            cloud::local_status().unwrap()["expired_without_acknowledgment_total"]
                .as_u64()
                .unwrap()
                >= 1
        );
        assert_no_request(&observer);
        eprintln!("generated-usage-custody stage=retention-complete");
        eprintln!(
            "Generated explicit V2 upload: exact lost-response replay, {} acknowledged events, open-hour/earlier-hour gaps, raw ACK preservation, unsent rolling expiry, and real scheduler wake/cancel passed",
            before_ack.len()
        );
    }
}
