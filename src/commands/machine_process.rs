//! Bounded subprocess ownership for explicitly selected machine execution.
use std::io;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CAPTURE_LIMIT: usize = 1024 * 1024;
static CHILDREN: Mutex<Vec<(u32, usize)>> = Mutex::new(Vec::new());

pub(crate) struct OwnedChild {
    pid: u32,
    handle: usize,
}
impl OwnedChild {
    pub(crate) fn attach_std(child: &std::process::Child) -> io::Result<Self> {
        let pid = child.id();
        #[cfg(windows)]
        let handle = {
            use std::os::windows::io::AsRawHandle;
            windows::attach_handle(child.as_raw_handle())?
        };
        #[cfg(not(windows))]
        let handle = 0;
        Self::register(pid, handle)
    }
    pub(crate) fn attach(child: &tokio::process::Child) -> io::Result<Self> {
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("Child identity unavailable"))?;
        #[cfg(windows)]
        let handle = windows::attach(child)?;
        #[cfg(not(windows))]
        let handle = 0;
        Self::register(pid, handle)
    }
    fn register(pid: u32, handle: usize) -> io::Result<Self> {
        let owned = Self { pid, handle };
        let mut children = CHILDREN.lock().unwrap_or_else(|e| e.into_inner());
        // Registration and the Windows resume are serialized with cancellation's
        // sweep. A child cannot start after that sweep has missed its identity.
        if super::machine::canceled() {
            drop(children);
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Invocation canceled",
            ));
        }
        children.push((pid, handle));
        #[cfg(windows)]
        if let Err(error) = windows::resume(pid) {
            drop(children);
            return Err(error);
        }
        drop(children);
        Ok(owned)
    }
    pub(crate) fn terminate(&self) -> io::Result<()> {
        terminate(self.pid, self.handle)
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.terminate();
        CHILDREN
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(pid, _)| *pid != self.pid);
        #[cfg(windows)]
        windows::close(self.handle);
    }
}
pub(crate) fn prepare(command: &mut tokio::process::Command) -> io::Result<()> {
    if super::machine::canceled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Invocation canceled",
        ));
    }
    command.kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(windows::CREATE_SUSPENDED);
    Ok(())
}

pub(crate) fn run_sync(command: &mut std::process::Command) -> io::Result<std::process::Output> {
    run_sync_with_input(command, Duration::from_secs(600), None)
}

pub(crate) fn run_sync_with_input(
    command: &mut std::process::Command,
    remaining: Duration,
    input: Option<&[u8]>,
) -> io::Result<std::process::Output> {
    use std::io::{Read, Write};
    let started = std::time::Instant::now();
    if super::machine::canceled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Invocation canceled",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows::CREATE_SUSPENDED);
    }
    if input.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let owned = match OwnedChild::attach_std(&child) {
        Ok(owned) => owned,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    fn read(mut stream: impl Read) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        stream
            .by_ref()
            .take(CAPTURE_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > CAPTURE_LIMIT {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Child output exceeded its bound",
            ))
        } else {
            Ok(bytes)
        }
    }
    enum PipeCompletion {
        Stdout(Vec<u8>),
        Stderr(Vec<u8>),
        Stdin,
    }
    let (completed, receiver) = std::sync::mpsc::channel::<io::Result<PipeCompletion>>();
    let operation = (|| {
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("Missing child stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("Missing child stderr"))?;
        let output_done = completed.clone();
        std::thread::Builder::new().spawn(move || {
            let _ = output_done.send(read(stdout).map(PipeCompletion::Stdout));
        })?;
        let error_done = completed.clone();
        std::thread::Builder::new().spawn(move || {
            let _ = error_done.send(read(stderr).map(PipeCompletion::Stderr));
        })?;
        let mut stdin_done = input.is_none();
        if let Some(input) = input {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("Missing child stdin"))?;
            let bytes = input.to_vec();
            let input_done = completed.clone();
            std::thread::Builder::new().spawn(move || {
                let result = stdin.write_all(&bytes);
                drop(stdin);
                let _ = input_done.send(result.map(|()| PipeCompletion::Stdin));
            })?;
        }
        drop(completed);
        let mut status = None;
        let mut stdout = None;
        let mut stderr = None;
        loop {
            if super::machine::canceled() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Invocation canceled",
                ));
            }
            if started.elapsed() >= remaining {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Child execution exceeded its deadline",
                ));
            }
            while let Ok(completion) = receiver.try_recv() {
                match completion? {
                    PipeCompletion::Stdout(bytes) => stdout = Some(bytes),
                    PipeCompletion::Stderr(bytes) => stderr = Some(bytes),
                    PipeCompletion::Stdin => stdin_done = true,
                }
            }
            if status.is_none() {
                status = child.try_wait()?;
                if status.is_some() {
                    // Descendants must not retain pipes after the direct child
                    // finishes. Pipe completion remains inside the same deadline.
                    owned.terminate()?;
                }
            }
            if let Some(status) = status {
                if stdin_done && stdout.is_some() && stderr.is_some() {
                    return Ok(std::process::Output {
                        status,
                        stdout: stdout.take().unwrap(),
                        stderr: stderr.take().unwrap(),
                    });
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    if operation.is_err() {
        let _ = owned.terminate();
        let _ = child.kill();
        let _ = child.wait();
    }
    operation
}
fn terminate(pid: u32, handle: usize) -> io::Result<()> {
    #[cfg(unix)]
    {
        let _ = handle;
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // Negative identities address the owned process group. ESRCH proves
        // absence; permission or other failures cannot establish cleanup.
        if unsafe { kill(-(pid as i32), 9) } == 0 {
            Ok(())
        } else {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(3) {
                Ok(())
            } else {
                Err(error)
            }
        }
    }
    #[cfg(windows)]
    {
        let _ = pid;
        windows::terminate(handle)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pid, handle);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Process ownership is unavailable",
        ))
    }
}
pub(crate) fn terminate_all() -> io::Result<()> {
    for &(pid, handle) in CHILDREN.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        terminate(pid, handle)?;
    }
    Ok(())
}
async fn bounded_read(mut stream: impl tokio::io::AsyncRead + Unpin) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    // Heap backing avoids multiplying this fixed buffer across nested async frames.
    let mut chunk = vec![0u8; 8192];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(bytes);
        }
        if bytes.len() + n > CAPTURE_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Child output exceeded its bound",
            ));
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
}
pub(crate) async fn wait(
    child: tokio::process::Child,
    remaining: Duration,
) -> io::Result<std::process::Output> {
    wait_with_optional_input(child, remaining, None).await
}

pub(crate) async fn wait_with_input(
    child: tokio::process::Child,
    remaining: Duration,
    input: &[u8],
) -> io::Result<std::process::Output> {
    wait_with_optional_input(child, remaining, Some(input)).await
}

async fn wait_with_optional_input(
    mut child: tokio::process::Child,
    remaining: Duration,
    input: Option<&[u8]>,
) -> io::Result<std::process::Output> {
    let deadline = tokio::time::Instant::now() + remaining;
    let owned = match OwnedChild::attach(&child) {
        Ok(owned) => owned,
        Err(error) => {
            let _ = child.kill().await;
            return Err(error);
        }
    };
    let operation = async {
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("Missing child stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("Missing child stderr"))?;
        let stdin = child.stdin.take();
        let send_input = async move {
            if let Some(input) = input {
                let mut stdin = stdin.ok_or_else(|| io::Error::other("Missing child stdin"))?;
                stdin.write_all(input).await?;
                stdin.shutdown().await?;
            }
            Ok::<(), io::Error>(())
        };
        // Drain both output channels while feeding stdin: tools may produce
        // output before consuming the complete request. One deadline covers all
        // pipes as well as process completion.
        let (status, stdout, stderr, ()) = tokio::try_join!(
            child.wait(),
            bounded_read(stdout),
            bounded_read(stderr),
            send_input
        )?;
        Ok(std::process::Output {
            status,
            stdout,
            stderr,
        })
    };
    match tokio::time::timeout_at(deadline, operation).await {
        Ok(result) => {
            let _ = owned.terminate();
            if result.is_err() {
                let _ = child.kill().await;
            }
            result
        }
        Err(_) => {
            let _ = owned.terminate();
            let _ = child.kill().await;
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Child execution exceeded its deadline",
            ))
        }
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;
    pub(super) const CREATE_SUSPENDED: u32 = 0x0000_0004;
    const TH32CS_SNAPTHREAD: u32 = 0x0000_0004;
    const THREAD_SUSPEND_RESUME: u32 = 0x0000_0002;
    const THREAD_QUERY_LIMITED_INFORMATION: u32 = 0x0000_0800;
    const ERROR_NO_MORE_FILES: i32 = 18;

    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    #[repr(C)]
    #[derive(Default)]
    struct ThreadEntry {
        size: u32,
        usage: u32,
        thread_id: u32,
        owner_process_id: u32,
        base_priority: i32,
        delta_priority: i32,
        flags: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Basic {
        process_time: i64,
        job_time: i64,
        flags: u32,
        min_working: usize,
        max_working: usize,
        active: u32,
        affinity: usize,
        priority: u32,
        scheduling: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Extended {
        basic: Basic,
        io: [u64; 6],
        process_memory: usize,
        job_memory: usize,
        peak_process: usize,
        peak_job: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: u32,
            info: *const c_void,
            length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn TerminateJobObject(job: *mut c_void, exit: u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> *mut c_void;
        fn Thread32First(snapshot: *mut c_void, entry: *mut ThreadEntry) -> i32;
        fn Thread32Next(snapshot: *mut c_void, entry: *mut ThreadEntry) -> i32;
        fn OpenThread(access: u32, inherit: i32, thread_id: u32) -> *mut c_void;
        fn GetProcessIdOfThread(thread: *mut c_void) -> u32;
        fn ResumeThread(thread: *mut c_void) -> u32;
    }
    pub(super) fn resume(pid: u32) -> io::Result<()> {
        // std/Tokio retain the process handle but close CreateProcess's initial
        // thread handle. The process is still CREATE_SUSPENDED, so enumerate its
        // sole initial thread and reopen only that thread before resuming it.
        // Do not request module snapshots: a suspended process has not loaded
        // its modules yet.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == (-1isize) as *mut c_void {
                return Err(io::Error::last_os_error());
            }
            let snapshot = Handle(snapshot);
            let mut entry = ThreadEntry {
                size: std::mem::size_of::<ThreadEntry>() as u32,
                ..Default::default()
            };
            if Thread32First(snapshot.0, &mut entry) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut initial_thread = None;
            loop {
                if entry.owner_process_id == pid {
                    if initial_thread.replace(entry.thread_id).is_some() {
                        return Err(io::Error::other(
                            "Suspended child thread identity is ambiguous",
                        ));
                    }
                }
                entry.size = std::mem::size_of::<ThreadEntry>() as u32;
                if Thread32Next(snapshot.0, &mut entry) == 0 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(ERROR_NO_MORE_FILES) {
                        return Err(error);
                    }
                    break;
                }
            }
            let thread_id = initial_thread
                .ok_or_else(|| io::Error::other("Suspended child thread is unavailable"))?;
            let thread = OpenThread(
                THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
                0,
                thread_id,
            );
            if thread.is_null() {
                return Err(io::Error::last_os_error());
            }
            let thread = Handle(thread);
            if GetProcessIdOfThread(thread.0) != pid {
                return Err(io::Error::other("Suspended child thread identity changed"));
            }
            match ResumeThread(thread.0) {
                1 => Ok(()),
                u32::MAX => Err(io::Error::last_os_error()),
                _ => Err(io::Error::other(
                    "Child did not resume from its initial suspension",
                )),
            }
        }
    }
    pub(super) fn attach(child: &tokio::process::Child) -> io::Result<usize> {
        let process = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("Child handle unavailable"))?;
        attach_handle(process)
    }
    pub(super) fn attach_handle(process: std::os::windows::io::RawHandle) -> io::Result<usize> {
        // Handles are kept under the ownership registry until guard cleanup.
        // The native job closes all assigned descendants when its owner drops.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut info = Extended::default();
            info.basic.flags = 0x2000;
            if SetInformationJobObject(
                job,
                9,
                &info as *const _ as *const c_void,
                std::mem::size_of::<Extended>() as u32,
            ) == 0
                || AssignProcessToJobObject(job, process) == 0
            {
                let error = io::Error::last_os_error();
                CloseHandle(job);
                return Err(error);
            }
            Ok(job as usize)
        }
    }
    pub(super) fn terminate(handle: usize) -> io::Result<()> {
        if unsafe { TerminateJobObject(handle as *mut c_void, 1) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    pub(super) fn close(handle: usize) {
        unsafe {
            CloseHandle(handle as *mut c_void);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    const FIXTURE_MODE: &str = "CARGO_AI_MACHINE_PROCESS_TEST_MODE";
    const FIXTURE_TEST: &str = "commands::machine_process::tests::owned_process_fixture";

    fn fixture_command(mode: &str) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", FIXTURE_TEST, "--nocapture"])
            .env(FIXTURE_MODE, mode)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        prepare(&mut command).unwrap();
        command
    }

    // The executable test fixture avoids relying on a shell, Python, or installed
    // helper programs. It runs only when explicitly selected by another test.
    #[test]
    fn owned_process_fixture() {
        let Ok(mode) = std::env::var(FIXTURE_MODE) else {
            return;
        };
        match mode.as_str() {
            "blocked_input" => std::thread::sleep(Duration::from_secs(30)),
            "excess_output" => {
                let _ = std::io::stdout().write_all(&vec![b'x'; CAPTURE_LIMIT + 8192]);
                std::thread::sleep(Duration::from_secs(30));
            }
            "output_before_input" => {
                std::io::stdout()
                    .write_all(&vec![b'o'; 128 * 1024])
                    .unwrap();
                std::io::stderr()
                    .write_all(&vec![b'e'; 128 * 1024])
                    .unwrap();
                let mut input = Vec::new();
                std::io::stdin().read_to_end(&mut input).unwrap();
                assert_eq!(input, vec![b'i'; 256 * 1024]);
                std::io::stdout().write_all(b"input-complete").unwrap();
            }
            #[cfg(windows)]
            "tree_root" => {
                let directory = std::path::PathBuf::from(
                    std::env::var_os("CARGO_AI_MACHINE_PROCESS_TEST_DIR").unwrap(),
                );
                std::fs::write(directory.join("started"), b"started").unwrap();
                let mut descendant = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", FIXTURE_TEST, "--nocapture"])
                    .env(FIXTURE_MODE, "tree_descendant")
                    .spawn()
                    .unwrap();
                descendant.wait().unwrap();
            }
            #[cfg(windows)]
            "tree_descendant" => {
                let directory = std::path::PathBuf::from(
                    std::env::var_os("CARGO_AI_MACHINE_PROCESS_TEST_DIR").unwrap(),
                );
                std::fs::write(directory.join("descendant"), std::process::id().to_string())
                    .unwrap();
                std::thread::sleep(Duration::from_secs(30));
            }
            _ => panic!("Unknown owned-process fixture mode"),
        }
    }

    #[tokio::test]
    async fn blocked_child_stdin_shares_execution_deadline() {
        let child = fixture_command("blocked_input").spawn().unwrap();
        let pid = child.id().unwrap();
        let start = std::time::Instant::now();
        let error = wait_with_input(child, Duration::from_millis(150), &vec![b'i'; 256 * 1024])
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(!CHILDREN
            .lock()
            .unwrap()
            .iter()
            .any(|(registered, _)| *registered == pid));
    }

    #[tokio::test]
    async fn child_output_is_drained_while_request_stdin_is_written() {
        let child = fixture_command("output_before_input").spawn().unwrap();
        let output = wait_with_input(child, Duration::from_secs(10), &vec![b'i'; 256 * 1024])
            .await
            .unwrap();
        assert!(output.status.success());
        assert!(output
            .stdout
            .windows(b"input-complete".len())
            .any(|bytes| bytes == b"input-complete"));
        assert_eq!(output.stderr, vec![b'e'; 128 * 1024]);
    }

    #[test]
    fn sync_child_output_is_drained_while_request_stdin_is_written() {
        let mut command = fixture_command("output_before_input").into_std();
        let output = run_sync_with_input(
            &mut command,
            Duration::from_secs(10),
            Some(&vec![b'i'; 256 * 1024]),
        )
        .unwrap();
        assert!(output.status.success());
        assert!(output
            .stdout
            .windows(b"input-complete".len())
            .any(|bytes| bytes == b"input-complete"));
        assert_eq!(output.stderr, vec![b'e'; 128 * 1024]);
    }

    #[test]
    fn sync_blocked_stdin_and_excess_output_fail_within_the_deadline() {
        let mut command = fixture_command("blocked_input").into_std();
        let start = std::time::Instant::now();
        let error = run_sync_with_input(
            &mut command,
            Duration::from_millis(150),
            Some(&vec![b'i'; 256 * 1024]),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(3));

        let mut command = fixture_command("excess_output").into_std();
        let start = std::time::Instant::now();
        let error = run_sync_with_input(&mut command, Duration::from_secs(10), None).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "Output failure was ignored until the process deadline"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_suspended_job_owns_fast_descendant_before_cleanup() {
        use std::ffi::c_void;
        #[link(name = "kernel32")]
        extern "system" {
            fn IsProcessInJob(process: *mut c_void, job: *mut c_void, result: *mut i32) -> i32;
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
            fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }
        struct ProcessHandle(*mut c_void);
        impl Drop for ProcessHandle {
            fn drop(&mut self) {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
        let directory =
            std::env::temp_dir().join(format!("cargo-ai-owned-tree-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let mut command = fixture_command("tree_root");
        command.env("CARGO_AI_MACHINE_PROCESS_TEST_DIR", &directory);
        let mut child = command.spawn().unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !directory.join("started").exists(),
            "The child ran before job assignment"
        );
        let owned = OwnedChild::attach(&child).unwrap();
        let mut assigned = 0;
        assert_ne!(
            unsafe {
                IsProcessInJob(
                    child.raw_handle().unwrap(),
                    owned.handle as *mut c_void,
                    &mut assigned,
                )
            },
            0
        );
        assert_ne!(assigned, 0);
        let pid = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(pid) = std::fs::read_to_string(directory.join("descendant"))
                    .ok()
                    .and_then(|text| text.parse::<u32>().ok())
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let descendant = ProcessHandle(unsafe { OpenProcess(0x0010_0000 | 0x1000, 0, pid) });
        assert!(!descendant.0.is_null());
        assigned = 0;
        assert_ne!(
            unsafe { IsProcessInJob(descendant.0, owned.handle as *mut c_void, &mut assigned) },
            0
        );
        assert_ne!(
            assigned, 0,
            "A descendant escaped the initial job assignment"
        );
        assert_eq!(unsafe { WaitForSingleObject(descendant.0, 0) }, 258);
        owned.terminate().unwrap();
        tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(descendant.0, 3000) },
            0,
            "Job cleanup left a descendant alive"
        );
        drop(owned);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_resume_failure_terminates_the_assigned_process() {
        // An unprepared child violates the suspended-launch invariant. Whether
        // the test harness has started additional threads or not, attachment
        // must fail closed instead of treating a running child as owned safely.
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", FIXTURE_TEST, "--nocapture"])
            .env(FIXTURE_MODE, "blocked_input")
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let child = command.spawn().unwrap();
        let pid = child.id().unwrap();
        let error = wait(child, Duration::from_secs(3)).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(!CHILDREN
            .lock()
            .unwrap()
            .iter()
            .any(|(registered, _)| *registered == pid));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn owned_tree_deadline_closes_inherited_pipes() {
        let mut command = tokio::process::Command::new("sh");
        command
            .args(["-c", "sleep 30 & wait"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        prepare(&mut command).unwrap();
        let start = std::time::Instant::now();
        let error = wait(command.spawn().unwrap(), Duration::from_millis(100))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
