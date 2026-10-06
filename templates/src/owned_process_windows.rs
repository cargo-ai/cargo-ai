//! Windows suspended-process job ownership shared with generated native execution.
use std::ffi::c_void;
use std::io;
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
