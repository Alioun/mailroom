//! Directory operations that preserve every entry: directories are held open and names are
//! resolved against those descriptors, a fresh directory must not already exist, and a move
//! refuses to replace whatever is at its destination. Nothing here unlinks a file.
use super::AcquisitionError;
use std::{
    ffi::{CString, c_char, c_int, c_uint},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::Path,
};
#[cfg(test)]
use zeroize::Zeroizing;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn renameatx_np(
        from_dir: c_int,
        from: *const c_char,
        to_dir: c_int,
        to: *const c_char,
        flags: c_uint,
    ) -> c_int;
}
/// `RENAME_EXCL` from `<sys/stdio.h>`: fail with `EEXIST` instead of replacing the destination.
#[cfg(target_os = "macos")]
const RENAME_EXCL: c_uint = 0x4;

fn name(text: &str) -> Result<CString, AcquisitionError> {
    if text.is_empty() || text.contains('/') || text == "." || text == ".." {
        return Err(AcquisitionError::Storage);
    }
    CString::new(text).map_err(|_| AcquisitionError::Storage)
}

fn errno() -> Option<c_int> {
    std::io::Error::last_os_error().raw_os_error()
}

/// How an exclusive move ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Moved {
    Moved,
    /// Nothing had the source name, so nothing moved.
    SourceMissing,
    /// Something already had the destination name; both entries are as they were.
    DestinationTaken,
}

/// An open directory. Names are resolved against it, never against a path.
pub(crate) struct Dir(OwnedFd);

impl Dir {
    /// Opens `path` as a directory, refusing a final symlink.
    pub(crate) fn open(path: &Path) -> Result<Self, AcquisitionError> {
        use std::os::unix::ffi::OsStrExt;
        let text =
            CString::new(path.as_os_str().as_bytes()).map_err(|_| AcquisitionError::Storage)?;
        let fd = unsafe {
            libc::open(
                text.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(AcquisitionError::Storage);
        }
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    /// Whether this directory has an entry `entry`, without following it.
    pub(crate) fn has(&self, entry: &str) -> Result<bool, AcquisitionError> {
        let entry = name(entry)?;
        let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
        if unsafe {
            libc::fstatat(
                self.0.as_raw_fd(),
                entry.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } == 0
        {
            return Ok(true);
        }
        match errno() {
            Some(libc::ENOENT) => Ok(false),
            _ => Err(AcquisitionError::Storage),
        }
    }

    /// Creates the subdirectory `entry`, which must not exist, with mode 0700, and opens it
    /// without following a link. Something substituted for it in between is refused.
    pub(crate) fn create_fresh(&self, entry: &str) -> Result<Self, AcquisitionError> {
        let text = name(entry)?;
        if unsafe { libc::mkdirat(self.0.as_raw_fd(), text.as_ptr(), 0o700) } != 0 {
            return Err(AcquisitionError::Storage);
        }
        #[cfg(test)]
        AFTER_CREATE.with(|hook| {
            if let Some(hook) = hook.borrow_mut().as_mut() {
                hook(entry);
            }
        });
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                text.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(AcquisitionError::Storage);
        }
        let created = Self(unsafe { OwnedFd::from_raw_fd(fd) });
        let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
        if unsafe { libc::fstat(created.0.as_raw_fd(), &mut stat) } != 0
            || stat.st_uid != unsafe { libc::geteuid() }
        {
            return Err(AcquisitionError::Storage);
        }
        Ok(created)
    }

    /// Reads the regular file `entry` through a descriptor opened without following a link.
    #[cfg(test)]
    pub(crate) fn read(
        &self,
        entry: &str,
        limit: u64,
    ) -> Result<Zeroizing<Vec<u8>>, AcquisitionError> {
        use std::io::Read;
        let text = name(entry)?;
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                text.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(match errno() {
                Some(libc::ENOENT) => AcquisitionError::CredentialMissing,
                _ => AcquisitionError::Storage,
            });
        }
        let file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        let mut bytes = Zeroizing::new(Vec::new());
        file.take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| AcquisitionError::Storage)?;
        if bytes.len() as u64 > limit {
            return Err(AcquisitionError::Storage);
        }
        Ok(bytes)
    }

    /// Flushes this directory's entries to storage.
    pub(crate) fn sync(&self) -> Result<(), AcquisitionError> {
        #[cfg(test)]
        {
            let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
            if unsafe { libc::fstat(self.0.as_raw_fd(), &mut stat) } == 0 {
                SYNCED.with(|synced| synced.borrow_mut().push((stat.st_dev as u64, stat.st_ino)));
            }
        }
        if unsafe { libc::fsync(self.0.as_raw_fd()) } != 0 {
            return Err(AcquisitionError::Storage);
        }
        Ok(())
    }
}

/// Moves `from/entry` to `to/destination` only if nothing has that destination name, and
/// never replaces it. Both names resolve against the open directories.
pub(crate) fn move_exclusive(
    from: &Dir,
    entry: &str,
    to: &Dir,
    destination: &str,
) -> Result<Moved, AcquisitionError> {
    let (source, target) = (name(entry)?, name(destination)?);
    #[cfg(target_os = "macos")]
    {
        if unsafe {
            renameatx_np(
                from.0.as_raw_fd(),
                source.as_ptr(),
                to.0.as_raw_fd(),
                target.as_ptr(),
                RENAME_EXCL,
            )
        } == 0
        {
            return Ok(Moved::Moved);
        }
        match errno() {
            Some(libc::ENOENT) => Ok(Moved::SourceMissing),
            Some(libc::EEXIST) => Ok(Moved::DestinationTaken),
            _ => Err(AcquisitionError::Storage),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (from, to, source, target);
        Err(AcquisitionError::Storage)
    }
}

#[cfg(test)]
thread_local! {
    /// Runs between creating a fresh directory and opening it, as another process could.
    pub(crate) static AFTER_CREATE: std::cell::RefCell<Option<Box<dyn FnMut(&str)>>> =
        std::cell::RefCell::new(None);
    /// Device and inode of every directory synced on this thread.
    pub(crate) static SYNCED: std::cell::RefCell<Vec<(u64, u64)>> = const { std::cell::RefCell::new(Vec::new()) };
}
