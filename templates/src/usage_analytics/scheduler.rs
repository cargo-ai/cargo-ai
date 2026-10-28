//! A scoped timer exists only while an actual operation is running.
use std::{future::Future, sync::Arc, time::Duration};
use tokio::task::JoinHandle;
pub(crate) fn canceled() -> bool {
    #[cfg(cargo_ai_cli)]
    {
        crate::commands::machine::canceled()
    }
    #[cfg(not(cargo_ai_cli))]
    {
        false
    }
}
struct Worker(Option<JoinHandle<()>>);
impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(worker) = self.0.take() {
            worker.abort();
        }
    }
}
async fn pass() {
    if canceled() {
        return;
    }
    let _ = super::opportunistic().await;
    if !canceled() {
        let _ = crate::usage_store::retention::apply(super::now_ms());
    }
}
/// Dropping the operation aborts its worker immediately; Drop never waits or
/// starts network I/O. Monotonic sleeps follow each settled pass, matching the
/// persisted completion-based deadline without skipping the following hour.
pub(crate) fn scope<F: Future>(future: F) -> impl Future<Output = F::Output> {
    // Pin the operation before async wrappers can duplicate its inline state.
    scope_with(Box::pin(future), Duration::from_secs(3600), pass)
}
pub(super) async fn scope_with<F, P, T>(future: F, period: Duration, pass: P) -> F::Output
where
    F: Future,
    P: Fn() -> T + Send + Sync + 'static,
    T: Future<Output = ()> + Send,
{
    let pass = Arc::new(pass);
    let worker_pass = pass.clone();
    let mut worker = Worker(Some(tokio::spawn(async move {
        loop {
            if canceled() {
                return;
            }
            worker_pass().await;
            tokio::time::sleep(period).await;
        }
    })));
    let output = future.await;
    if let Some(handle) = worker.0.take() {
        handle.abort();
        let _ = handle.await;
    }
    if !canceled() {
        pass().await;
    }
    output
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn large_operation_state_is_bounded_and_deferred_until_poll() {
        let started = AtomicUsize::new(0);
        let observed = &started;
        let payload = [0u8; 64 * 1024];
        let operation = async move {
            observed.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<()>().await;
            std::hint::black_box(payload);
        };
        assert!(std::mem::size_of_val(&operation) >= 64 * 1024);
        let scoped = scope(operation);
        assert!(std::mem::size_of_val(&scoped) < 8 * 1024);
        assert_eq!(started.load(Ordering::SeqCst), 0);
        drop(scoped);
        assert_eq!(started.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn real_timer_wakes_during_active_work_and_stops_at_scope_end() {
        let count = Arc::new(AtomicUsize::new(0));
        let observed = count.clone();
        scope_with(
            tokio::time::sleep(Duration::from_millis(45)),
            Duration::from_millis(10),
            move || {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                }
            },
        )
        .await;
        let stopped = observed.load(Ordering::SeqCst);
        assert!(stopped >= 4);
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(observed.load(Ordering::SeqCst), stopped);
    }
    #[tokio::test]
    #[ignore = "launched explicitly by the real process cancellation test"]
    async fn process_timer_child() {
        let count = Arc::new(AtomicUsize::new(0));
        let observed = count.clone();
        let worker = tokio::spawn(scope_with(
            std::future::pending::<()>(),
            Duration::from_millis(10),
            move || {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                }
            },
        ));
        tokio::time::sleep(Duration::from_millis(45)).await;
        worker.abort();
        let _ = worker.await;
        let stopped = observed.load(Ordering::SeqCst);
        assert!(stopped >= 3);
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(observed.load(Ordering::SeqCst), stopped);
    }
    #[test]
    fn real_process_cancellation_stops_the_production_scoped_timer() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "usage_analytics::scheduler::tests::process_timer_child",
                "--ignored",
                "--nocapture",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
}
