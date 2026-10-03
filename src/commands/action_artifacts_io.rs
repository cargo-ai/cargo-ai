//! Open-handle confinement for passive artifact reads; never creates paths.
use super::{changed, too_large, unsafe_file, ArtifactError, Result};
use std::{
    fs::File,
    io::Read,
    path::{Component, Path},
};

fn io_error(error: std::io::Error) -> ArtifactError {
    if error.kind() == std::io::ErrorKind::NotFound {
        super::error("artifact.not_found", "The artifact is no longer available.")
    } else {
        unsafe_file()
    }
}
pub(super) fn read(root: &Path, relative: &str, limit: usize) -> Result<Vec<u8>> {
    read_checked(root, relative, limit, || {})
}
fn read_checked(
    root: &Path,
    relative: &str,
    limit: usize,
    after_open: impl FnOnce(),
) -> Result<Vec<u8>> {
    if !root.is_absolute()
        || root
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err(unsafe_file());
    }
    // Both root and every descendant are opened without following links. The caller
    // also binds the effective ownership/configuration identity to the request.
    let opened = platform::Opened::open(root, relative)?;
    let before = platform::stamp(&opened.file)?;
    if before.length > limit as u64 {
        return Err(too_large());
    }
    after_open();
    let mut bytes = Vec::with_capacity(before.length as usize);
    (&opened.file)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unsafe_file())?;
    if bytes.len() > limit {
        return Err(too_large());
    }
    let after = platform::stamp(&opened.file)?;
    if before != after || after.length != bytes.len() as u64 {
        return Err(changed());
    }
    opened.recheck()?;
    Ok(bytes)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod platform {
    use super::*;
    use std::{
        ffi::CString,
        fs::Metadata,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::MetadataExt},
        },
    };
    unsafe extern "C" {
        fn openat(fd: i32, path: *const std::ffi::c_char, flags: i32, ...) -> i32;
    }
    #[cfg(target_os = "linux")]
    const FLAGS: i32 = 0x20000 | 0x80000 | 0x800; // NOFOLLOW | CLOEXEC | NONBLOCK
    #[cfg(target_os = "linux")]
    const DIRECTORY: i32 = 0x10000;
    #[cfg(target_os = "macos")]
    const FLAGS: i32 = 0x100 | 0x1000000 | 0x4;
    #[cfg(target_os = "macos")]
    const DIRECTORY: i32 = 0x100000;
    #[derive(PartialEq, Eq)]
    pub(super) struct Stamp {
        device: u64,
        inode: u64,
        pub length: u64,
        modified: (i64, i64),
        changed: (i64, i64),
        mode: u32,
        links: u64,
    }
    fn identity(metadata: &Metadata) -> (u64, u64) {
        (metadata.dev(), metadata.ino())
    }
    pub(super) fn stamp(file: &File) -> Result<Stamp> {
        let metadata = file.metadata().map_err(io_error)?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(unsafe_file());
        }
        Ok(Stamp {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            mode: metadata.mode(),
            links: metadata.nlink(),
        })
    }
    fn child(parent: &File, name: &std::ffi::OsStr, directory: bool) -> Result<File> {
        let name = CString::new(name.as_bytes()).map_err(|_| unsafe_file())?;
        // SAFETY: the borrowed directory descriptor and NUL-terminated name remain
        // valid for this call. O_CREAT is absent; the returned descriptor is owned.
        let fd = unsafe {
            openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                FLAGS | if directory { DIRECTORY } else { 0 },
            )
        };
        if fd < 0 {
            return Err(io_error(std::io::Error::last_os_error()));
        }
        // SAFETY: openat returned a fresh owned descriptor, transferred once.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    pub(super) struct Opened {
        pub file: File,
        anchors: Vec<File>,
        names: Vec<std::ffi::OsString>,
        leaf: std::ffi::OsString,
    }
    impl Opened {
        pub(super) fn open(root: &Path, relative: &str) -> Result<Self> {
            let mut names = Vec::new();
            for component in root.components() {
                match component {
                    Component::RootDir => {}
                    Component::Normal(value) => names.push(value.to_os_string()),
                    _ => return Err(unsafe_file()),
                }
            }
            for component in Path::new(relative).components() {
                match component {
                    Component::Normal(value) => names.push(value.to_os_string()),
                    _ => return Err(unsafe_file()),
                }
            }
            let leaf = names.pop().ok_or_else(unsafe_file)?;
            let mut anchors = vec![File::open("/").map_err(io_error)?];
            for name in &names {
                let directory = child(anchors.last().unwrap(), name, true)?;
                if !directory.metadata().map_err(io_error)?.is_dir() {
                    return Err(unsafe_file());
                }
                anchors.push(directory);
            }
            let file = child(anchors.last().unwrap(), &leaf, false)?;
            stamp(&file)?;
            Ok(Self {
                file,
                anchors,
                names,
                leaf,
            })
        }
        pub(super) fn recheck(&self) -> Result<()> {
            let mut parent = File::open("/").map_err(io_error)?;
            if identity(&parent.metadata().map_err(io_error)?)
                != identity(&self.anchors[0].metadata().map_err(io_error)?)
            {
                return Err(changed());
            }
            for (index, name) in self.names.iter().enumerate() {
                let reopened = child(&parent, name, true).map_err(|_| changed())?;
                if identity(&reopened.metadata().map_err(io_error)?)
                    != identity(&self.anchors[index + 1].metadata().map_err(io_error)?)
                {
                    return Err(changed());
                }
                parent = reopened;
            }
            let reopened = child(&parent, &self.leaf, false).map_err(|_| changed())?;
            if stamp(&reopened)? != stamp(&self.file)? {
                return Err(changed());
            }
            Ok(())
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{
        fs::OpenOptions,
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
        path::PathBuf,
    };
    #[repr(C)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[repr(C)]
    struct FileInformation {
        attributes: u32,
        creation: FileTime,
        access: FileTime,
        write: FileTime,
        volume: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(
            file: *mut std::ffi::c_void,
            information: *mut FileInformation,
        ) -> i32;
        fn GetFileType(file: *mut std::ffi::c_void) -> u32;
    }
    fn information(file: &File) -> Result<FileInformation> {
        let mut data = std::mem::MaybeUninit::uninit();
        // SAFETY: a live file handle and suitably sized output allocation are supplied.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), data.as_mut_ptr()) } == 0 {
            return Err(unsafe_file());
        }
        // SAFETY: successful API call initialized the complete structure.
        let data = unsafe { data.assume_init() };
        // Reject reparse points, device attributes and non-disk handles.
        if data.attributes & (0x400 | 0x40) != 0
            || unsafe { GetFileType(file.as_raw_handle()) } != 1
        {
            return Err(unsafe_file());
        }
        Ok(data)
    }
    #[derive(PartialEq, Eq)]
    pub(super) struct Stamp {
        volume: u32,
        index: (u32, u32),
        pub length: u64,
        created: FileTime,
        written: FileTime,
        attributes: u32,
        links: u32,
    }
    pub(super) fn stamp(file: &File) -> Result<Stamp> {
        let info = information(file)?;
        if info.attributes & 0x10 != 0 || info.links != 1 {
            return Err(unsafe_file());
        }
        Ok(Stamp {
            volume: info.volume,
            index: (info.index_high, info.index_low),
            length: ((info.size_high as u64) << 32) | info.size_low as u64,
            created: info.creation,
            written: info.write,
            attributes: info.attributes,
            links: info.links,
        })
    }
    fn open(path: &Path, directory: bool) -> Result<File> {
        // Deny write/delete sharing for every retained ancestor and file. This
        // prevents both renames and concurrent reparse-metadata write handles.
        let file = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .custom_flags(0x00200000 | 0x02000000)
            .open(path)
            .map_err(io_error)?;
        let info = information(&file)?;
        if (info.attributes & 0x10 != 0) != directory {
            return Err(unsafe_file());
        }
        Ok(file)
    }
    pub(super) struct Opened {
        pub file: File,
        _anchors: Vec<File>,
        path: PathBuf,
    }
    impl Opened {
        pub(super) fn open(root: &Path, relative: &str) -> Result<Self> {
            let mut path = PathBuf::new();
            let mut anchors = Vec::new();
            let mut parts = root.components();
            match parts.next() {
                Some(Component::Prefix(prefix)) => match prefix.kind() {
                    std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_) => {
                        path.push(prefix.as_os_str())
                    }
                    _ => return Err(unsafe_file()),
                },
                _ => return Err(unsafe_file()),
            }
            if !matches!(parts.next(), Some(Component::RootDir)) {
                return Err(unsafe_file());
            }
            path.push("\\");
            anchors.push(open(&path, true)?);
            for part in parts {
                if let Component::Normal(name) = part {
                    path.push(name);
                    anchors.push(open(&path, true)?);
                } else {
                    return Err(unsafe_file());
                }
            }
            let names = Path::new(relative).components().collect::<Vec<_>>();
            for (index, part) in names.iter().enumerate() {
                if let Component::Normal(name) = part {
                    path.push(name);
                } else {
                    return Err(unsafe_file());
                }
                if index + 1 < names.len() {
                    anchors.push(open(&path, true)?);
                }
            }
            let file = open(&path, false)?;
            stamp(&file)?;
            Ok(Self {
                file,
                _anchors: anchors,
                path,
            })
        }
        pub(super) fn recheck(&self) -> Result<()> {
            // Retained ancestors deny renames; a second handle verifies final identity.
            let again = open(&self.path, false)?;
            if stamp(&again)? != stamp(&self.file)? {
                return Err(changed());
            }
            Ok(())
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use super::*;
    #[derive(PartialEq, Eq)]
    pub(super) struct Stamp {
        pub length: u64,
    }
    pub(super) fn stamp(_: &File) -> Result<Stamp> {
        Err(super::super::error(
            "artifact.unsafe_file",
            "This platform cannot establish artifact confinement.",
        ))
    }
    pub(super) struct Opened {
        pub file: File,
    }
    impl Opened {
        pub(super) fn open(_: &Path, _: &str) -> Result<Self> {
            Err(unsafe_file())
        }
        pub(super) fn recheck(&self) -> Result<()> {
            Err(unsafe_file())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacement_and_mutation_during_read_are_rejected() {
        let root =
            std::env::temp_dir().join(format!("cargo-artifact-race-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let file = root.join("value.txt");
        std::fs::write(&file, "original").unwrap();
        #[cfg(unix)]
        {
            let result = read_checked(&root, "value.txt", 100, || {
                std::fs::rename(&file, root.join("old.txt")).unwrap();
                std::fs::write(&file, "replaced").unwrap();
            });
            assert_eq!(result.unwrap_err().code, "artifact.content_changed");
            let result = read_checked(&root, "value.txt", 100, || {
                std::fs::write(&file, "mutated").unwrap();
            });
            assert_eq!(result.unwrap_err().code, "artifact.content_changed");
            let result = read_checked(&root, "value.txt", 100, || {
                std::fs::rename(&root, root.with_extension("moved")).unwrap();
                std::fs::create_dir(&root).unwrap();
                std::fs::write(&file, "replaced").unwrap();
            });
            assert_eq!(result.unwrap_err().code, "artifact.content_changed");
            std::fs::remove_dir_all(root.with_extension("moved")).unwrap();
        }
        #[cfg(windows)]
        {
            let bytes = read_checked(&root, "value.txt", 100, || {
                assert!(std::fs::rename(&file, root.join("old.txt")).is_err());
                assert!(std::fs::write(&file, "changed").is_err());
                assert!(std::fs::rename(&root, root.with_extension("moved")).is_err());
            })
            .unwrap();
            assert_eq!(bytes, b"original");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}
