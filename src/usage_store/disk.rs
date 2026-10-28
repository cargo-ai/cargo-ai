//! Conservative available-space query using the supported platform's native ABI.
use std::path::Path;

#[cfg(any(
    target_os = "macos",
    all(
        target_os = "linux",
        target_pointer_width = "64",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        any(target_env = "gnu", target_env = "musl")
    )
))]
pub(super) fn available(path: &Path) -> Result<u64, String> {
    use std::ffi::{c_char, c_int, c_ulong, CString};
    use std::os::unix::ffi::OsStrExt;
    #[cfg(target_os = "macos")]
    type Count = u32;
    #[cfg(target_os = "linux")]
    type Count = u64;
    // Darwin's fs{blk,fil}cnt_t is unsigned int; supported Linux64 ABIs use
    // unsigned long. The trailing Linux reserved fields are part of the ABI.
    #[repr(C)]
    struct StatVfs {
        bsize: c_ulong,
        frsize: c_ulong,
        blocks: Count,
        bfree: Count,
        bavail: Count,
        files: Count,
        ffree: Count,
        favail: Count,
        fsid: c_ulong,
        flag: c_ulong,
        namemax: c_ulong,
        #[cfg(target_os = "linux")]
        spare: [c_int; 6],
    }
    unsafe extern "C" {
        fn statvfs(path: *const c_char, output: *mut StatVfs) -> c_int;
    }
    let path =
        CString::new(path.as_os_str().as_bytes()).map_err(|_| "Usage storage path is invalid")?;
    let mut result = std::mem::MaybeUninit::<StatVfs>::uninit();
    // The C function initializes the complete platform struct on success.
    if unsafe { statvfs(path.as_ptr(), result.as_mut_ptr()) } != 0 {
        return Err(
            "Cannot determine free space for usage migration; source left unchanged".into(),
        );
    }
    let result = unsafe { result.assume_init() };
    (result.bavail as u64)
        .checked_mul(result.frsize)
        .ok_or_else(|| "Usage migration free-space count overflowed".into())
}

#[cfg(windows)]
pub(super) fn available(path: &Path) -> Result<u64, String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(
            path: *const u16,
            available: *mut u64,
            total: *mut u64,
            free: *mut u64,
        ) -> i32;
    }
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0;
    if unsafe {
        GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(
            "Cannot determine free space for usage migration; source left unchanged".into(),
        );
    }
    Ok(available)
}

#[cfg(not(any(
    windows,
    target_os = "macos",
    all(
        target_os = "linux",
        target_pointer_width = "64",
        any(target_arch = "x86_64", target_arch = "aarch64"),
        any(target_env = "gnu", target_env = "musl")
    )
)))]
pub(super) fn available(_path: &Path) -> Result<u64, String> {
    Err("This platform cannot verify usage migration disk headroom; source left unchanged".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn available_space_is_reported_for_fixture_volume() {
        match super::available(&std::env::temp_dir()) {
            Ok(bytes) => assert!(bytes > 0),
            Err(error) => assert!(error.contains("This platform cannot verify"), "{error}"),
        }
    }
}
