//! Opt-in diagnostics for existing preparation work. Labels never contain caller data.
//! Read/hash counters belong only to their innermost span. Inclusive times overlap;
//! exclusive times subtract nested spans and can be added without double counting.

#[cfg_attr(not(feature = "preparation-metrics"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "preparation-metrics", derive(serde::Serialize))]
#[cfg_attr(feature = "preparation-metrics", serde(rename_all = "snake_case"))]
pub(crate) enum Stage {
    LoadTarget,
    InstalledAuthorityLease,
    Discover,
    Prepare,
    LoadCatalog,
    CatalogRead,
    DefinitionResourceReads,
    RoleInventory,
    VerifiedRoleInventory,
    ChildIdentityHash,
    ChildArtifactScan,
    ChildCapabilityScan,
    ChildDefinitionScan,
    ToolInventory,
    ToolSourceScan,
    FinalBinding,
    ResolveRoles,
    ResolveRolesFromContext,
    ProfileValidation,
}

#[cfg(feature = "preparation-metrics")]
mod enabled {
    use super::Stage;
    use serde::Serialize;
    use std::{cell::RefCell, collections::BTreeMap, io::Write, time::Instant};

    #[derive(Clone, Debug, Default, Serialize)]
    pub(crate) struct Metrics {
        pub(crate) invocations: u64,
        pub(crate) inclusive_ns: u64,
        pub(crate) exclusive_ns: u64,
        pub(crate) read_attempts: u64,
        pub(crate) files_opened: u64,
        pub(crate) read_bytes: u64,
        pub(crate) hash_calls: u64,
        pub(crate) hash_bytes: u64,
        pub(crate) checks: u64,
    }

    #[derive(Clone, Debug, Serialize)]
    pub(crate) struct Report {
        schema_version: u8,
        diagnostic: &'static str,
        operation: &'static str,
        pub(crate) elapsed_ns: u64,
        pub(crate) stages: BTreeMap<Stage, Metrics>,
    }

    struct Frame {
        stage: Stage,
        started: Instant,
        children_ns: u64,
    }
    struct State {
        report: Report,
        started: Instant,
        stack: Vec<Frame>,
    }
    thread_local! {
        static ACTIVE: RefCell<Option<State>> = const { RefCell::new(None) };
    }

    pub(crate) struct Session {
        owned: bool,
        emit: bool,
    }
    impl Session {
        /// Only passive metadata commands may opt in; normal builds have no collector.
        pub(crate) fn from_env(command: &str) -> Self {
            let operation = match command {
                "list" => "actions_list",
                "resolve" => "actions_resolve",
                _ => {
                    return Self {
                        owned: false,
                        emit: false,
                    }
                }
            };
            if std::env::var_os("CARGO_AI_PREPARATION_METRICS").as_deref()
                != Some(std::ffi::OsStr::new("1"))
            {
                return Self {
                    owned: false,
                    emit: false,
                };
            }
            Self::begin(operation, true)
        }
        fn begin(operation: &'static str, emit: bool) -> Self {
            let owned = ACTIVE.with(|active| {
                let mut active = active.borrow_mut();
                if active.is_some() {
                    return false;
                }
                *active = Some(State {
                    report: Report {
                        schema_version: 1,
                        diagnostic: "preparation_metrics",
                        operation,
                        elapsed_ns: 0,
                        stages: BTreeMap::new(),
                    },
                    started: Instant::now(),
                    stack: Vec::new(),
                });
                true
            });
            Self { owned, emit }
        }
        #[cfg(test)]
        pub(crate) fn capture() -> Self {
            Self::begin("test_preparation", false)
        }
        fn take(&mut self) -> Option<Report> {
            if !std::mem::take(&mut self.owned) {
                return None;
            }
            ACTIVE
                .with(|active| active.borrow_mut().take())
                .map(|mut state| {
                    state.report.elapsed_ns = nanos(state.started.elapsed());
                    state.report
                })
        }
        #[cfg(test)]
        pub(crate) fn finish(mut self) -> Report {
            self.take().expect("the test owns its measurement session")
        }
    }
    impl Drop for Session {
        fn drop(&mut self) {
            if let Some(report) = self.take() {
                if self.emit {
                    if let Ok(json) = serde_json::to_string(&report) {
                        // A closed diagnostic pipe must never change command behavior.
                        let _ = writeln!(std::io::stderr().lock(), "{json}");
                    }
                }
            }
        }
    }
    fn nanos(duration: std::time::Duration) -> u64 {
        duration.as_nanos().min(u64::MAX as u128) as u64
    }
    pub(crate) struct Span(bool);
    pub(crate) fn span(stage: Stage) -> Span {
        Span(ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            let Some(state) = active.as_mut() else {
                return false;
            };
            state.report.stages.entry(stage).or_default().invocations += 1;
            state.stack.push(Frame {
                stage,
                started: Instant::now(),
                children_ns: 0,
            });
            true
        }))
    }
    impl Drop for Span {
        fn drop(&mut self) {
            if !self.0 {
                return;
            }
            ACTIVE.with(|active| {
                let mut active = active.borrow_mut();
                let Some(state) = active.as_mut() else { return };
                let Some(frame) = state.stack.pop() else {
                    return;
                };
                let elapsed = nanos(frame.started.elapsed());
                let metrics = state.report.stages.entry(frame.stage).or_default();
                metrics.inclusive_ns = metrics.inclusive_ns.saturating_add(elapsed);
                metrics.exclusive_ns = metrics
                    .exclusive_ns
                    .saturating_add(elapsed.saturating_sub(frame.children_ns));
                if let Some(parent) = state.stack.last_mut() {
                    parent.children_ns = parent.children_ns.saturating_add(elapsed);
                }
            });
        }
    }
    fn current(update: impl FnOnce(&mut Metrics)) {
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            if let Some(state) = active.as_mut() {
                if let Some(frame) = state.stack.last() {
                    update(state.report.stages.entry(frame.stage).or_default());
                }
            }
        });
    }
    pub(crate) fn read_attempt() {
        current(|m| m.read_attempts += 1);
    }
    pub(crate) fn file_opened() {
        current(|m| m.files_opened += 1);
    }
    pub(crate) fn read_bytes(bytes: usize) {
        current(|m| m.read_bytes += bytes as u64);
    }
    pub(crate) fn hashed(bytes: usize) {
        current(|m| {
            m.hash_calls += 1;
            m.hash_bytes += bytes as u64;
        });
    }
    pub(crate) fn check(stage: Stage) {
        ACTIVE.with(|active| {
            if let Some(state) = active.borrow_mut().as_mut() {
                state.report.stages.entry(stage).or_default().checks += 1;
            }
        });
    }
}

#[cfg(not(feature = "preparation-metrics"))]
mod disabled {
    use super::Stage;
    pub(crate) struct Session;
    impl Session {
        #[inline(always)]
        pub(crate) fn from_env(_: &str) -> Self {
            Self
        }
    }
    pub(crate) struct Span;
    #[inline(always)]
    pub(crate) fn span(_: Stage) -> Span {
        Span
    }
    #[inline(always)]
    pub(crate) fn read_attempt() {}
    #[inline(always)]
    pub(crate) fn file_opened() {}
    #[inline(always)]
    pub(crate) fn read_bytes(_: usize) {}
    #[inline(always)]
    pub(crate) fn hashed(_: usize) {}
    #[inline(always)]
    pub(crate) fn check(_: Stage) {}
}

#[cfg(not(feature = "preparation-metrics"))]
pub(crate) use disabled::*;
#[cfg(feature = "preparation-metrics")]
pub(crate) use enabled::*;
