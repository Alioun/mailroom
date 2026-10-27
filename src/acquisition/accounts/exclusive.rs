//! Directory operations that preserve every entry: directories are held open and names are
//! resolved against those descriptors, a new directory never takes an existing name, and a
//! move refuses to replace whatever is at its destination. Nothing here unlinks anything.
use super::AcquisitionError;
use sha2::{Digest, Sha256};
use std::{
    ffi::{CStr, CString, c_int},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::Path,
};
#[cfg(test)]
use zeroize::Zeroizing;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn renameatx_np(
        from_dir: c_int,
        from: *const std::ffi::c_char,
        to_dir: c_int,
        to: *const std::ffi::c_char,
        flags: std::ffi::c_uint,
    ) -> c_int;
}
/// `RENAME_EXCL` from `<sys/stdio.h>`: fail with `EEXIST` instead of replacing the destination.
#[cfg(target_os = "macos")]
const RENAME_EXCL: std::ffi::c_uint = 0x4;

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

    fn stat_at(&self, entry: &CStr) -> Result<Option<libc::stat>, AcquisitionError> {
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
            return Ok(Some(stat));
        }
        match errno() {
            Some(libc::ENOENT) => Ok(None),
            _ => Err(AcquisitionError::Storage),
        }
    }

    /// Whether this directory has an entry `entry`, without following it.
    pub(crate) fn has(&self, entry: &str) -> Result<bool, AcquisitionError> {
        Ok(self.stat_at(&name(entry)?)?.is_some())
    }

    /// Creates the subdirectory `entry` with mode 0700. Returns false, creating nothing,
    /// when something already has that name.
    pub(crate) fn make(&self, entry: &str) -> Result<bool, AcquisitionError> {
        let text = name(entry)?;
        if unsafe { libc::mkdirat(self.0.as_raw_fd(), text.as_ptr(), 0o700) } != 0 {
            return match errno() {
                Some(libc::EEXIST) => Ok(false),
                _ => Err(AcquisitionError::Storage),
            };
        }
        #[cfg(test)]
        AFTER_CREATE.with(|hook| {
            if let Some(hook) = hook.borrow_mut().as_mut() {
                hook(entry);
            }
        });
        Ok(true)
    }

    /// Opens the subdirectory `entry` without following a link, and refuses one the
    /// effective user does not own. This does not prove which directory it is: a same-user
    /// process can put another directory of its own under that name first.
    pub(crate) fn open_dir(&self, entry: &str) -> Result<Self, AcquisitionError> {
        let opened = self.open_at(&name(entry)?, libc::O_DIRECTORY)?;
        let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
        if unsafe { libc::fstat(opened.as_raw_fd(), &mut stat) } != 0
            || stat.st_uid != unsafe { libc::geteuid() }
        {
            return Err(AcquisitionError::Storage);
        }
        Ok(Self(opened))
    }

    /// Creates the subdirectory `entry`, which must not exist, and opens it as `open_dir`
    /// does. The name being taken refuses.
    #[cfg(test)]
    pub(crate) fn create_fresh(&self, entry: &str) -> Result<Self, AcquisitionError> {
        if !self.make(entry)? {
            return Err(AcquisitionError::Storage);
        }
        self.open_dir(entry)
    }

    fn open_at(&self, entry: &CStr, flags: c_int) -> Result<OwnedFd, AcquisitionError> {
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                entry.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | flags,
            )
        };
        if fd < 0 {
            return Err(match errno() {
                Some(libc::ENOENT) => AcquisitionError::CredentialMissing,
                _ => AcquisitionError::Storage,
            });
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Opens the regular file `entry` without following a link or blocking on a FIFO, and
    /// refuses anything else.
    fn open_file(&self, entry: &CStr) -> Result<(std::fs::File, libc::stat), AcquisitionError> {
        let file = std::fs::File::from(self.open_at(entry, libc::O_NONBLOCK)?);
        let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
        if unsafe { libc::fstat(file.as_raw_fd(), &mut stat) } != 0
            || stat.st_mode & libc::S_IFMT != libc::S_IFREG
        {
            return Err(AcquisitionError::Storage);
        }
        Ok((file, stat))
    }

    /// Reads the regular file `entry` through a descriptor opened without following a link.
    #[cfg(test)]
    pub(crate) fn read(
        &self,
        entry: &str,
        limit: u64,
    ) -> Result<Zeroizing<Vec<u8>>, AcquisitionError> {
        use std::io::Read;
        let (file, _) = self.open_file(&name(entry)?)?;
        let mut bytes = Zeroizing::new(Vec::new());
        file.take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| AcquisitionError::Storage)?;
        if bytes.len() as u64 > limit {
            return Err(AcquisitionError::Storage);
        }
        Ok(bytes)
    }

    /// The names in this directory, read through a second descriptor of the same directory.
    fn names(&self) -> Result<Vec<Vec<u8>>, AcquisitionError> {
        let fd = self.open_at(c".", libc::O_DIRECTORY)?;
        let stream = unsafe { libc::fdopendir(fd.as_raw_fd()) };
        if stream.is_null() {
            return Err(AcquisitionError::Storage);
        }
        // The stream owns the descriptor from here on, and closedir closes it.
        std::mem::forget(fd);
        let mut names = vec![];
        let result = loop {
            clear_errno();
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                break match errno() {
                    Some(0) | None => Ok(()),
                    _ => Err(AcquisitionError::Storage),
                };
            }
            let text = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if text != b"." && text != b".." {
                names.push(text.to_vec());
            }
        };
        unsafe { libc::closedir(stream) };
        result.map(|()| {
            names.sort();
            names
        })
    }

    /// Adds the entry `entry`, and everything under it, to `hash`, without following a
    /// link, and returns the bytes of the regular files in it. Each entry contributes its
    /// type, its name's length and bytes, and then a file's length and SHA-256, a link's
    /// target, or a directory's entries in byte order followed by an end mark. Any other
    /// type, a file over `limit`, or more than `budget` entries refuses.
    pub(crate) fn digest(
        &self,
        entry: &str,
        limit: u64,
        budget: &mut usize,
        hash: &mut Sha256,
    ) -> Result<u64, AcquisitionError> {
        self.digest_entry(&name(entry)?, limit, budget, hash)
    }

    fn digest_entry(
        &self,
        entry: &CStr,
        limit: u64,
        budget: &mut usize,
        hash: &mut Sha256,
    ) -> Result<u64, AcquisitionError> {
        use std::io::Read;
        *budget = budget.checked_sub(1).ok_or(AcquisitionError::Storage)?;
        let stat = self.stat_at(entry)?.ok_or(AcquisitionError::Storage)?;
        let field = |hash: &mut Sha256, bytes: &[u8]| {
            hash.update((bytes.len() as u64).to_be_bytes());
            hash.update(bytes);
        };
        let same = |other: &libc::stat| other.st_dev == stat.st_dev && other.st_ino == stat.st_ino;
        match stat.st_mode & libc::S_IFMT {
            libc::S_IFREG => {
                let (file, opened) = self.open_file(entry)?;
                if !same(&opened) {
                    return Err(AcquisitionError::Storage);
                }
                let mut bytes = Vec::new();
                file.take(limit + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| AcquisitionError::Storage)?;
                if bytes.len() as u64 > limit {
                    return Err(AcquisitionError::Storage);
                }
                hash.update(b"f");
                field(hash, entry.to_bytes());
                hash.update((bytes.len() as u64).to_be_bytes());
                hash.update(Sha256::digest(&bytes));
                Ok(bytes.len() as u64)
            }
            libc::S_IFLNK => {
                let mut target = vec![0u8; 4096];
                let length = unsafe {
                    libc::readlinkat(
                        self.0.as_raw_fd(),
                        entry.as_ptr(),
                        target.as_mut_ptr().cast(),
                        target.len(),
                    )
                };
                if length < 0 || length as usize >= target.len() {
                    return Err(AcquisitionError::Storage);
                }
                hash.update(b"l");
                field(hash, entry.to_bytes());
                field(hash, &target[..length as usize]);
                Ok(0)
            }
            libc::S_IFDIR => {
                let child = Self(self.open_at(entry, libc::O_DIRECTORY)?);
                let mut opened = unsafe { std::mem::zeroed::<libc::stat>() };
                if unsafe { libc::fstat(child.0.as_raw_fd(), &mut opened) } != 0 || !same(&opened) {
                    return Err(AcquisitionError::Storage);
                }
                hash.update(b"d");
                field(hash, entry.to_bytes());
                let mut total = 0u64;
                for name in child.names()? {
                    let name = CString::new(name).map_err(|_| AcquisitionError::Storage)?;
                    total += child.digest_entry(&name, limit, budget, hash)?;
                }
                hash.update(b"e");
                Ok(total)
            }
            _ => Err(AcquisitionError::Storage),
        }
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

fn clear_errno() {
    #[cfg(target_os = "macos")]
    unsafe {
        *libc::__error() = 0;
    }
    #[cfg(target_os = "linux")]
    unsafe {
        *libc::__errno_location() = 0;
    }
}

/// Moves `from/entry` to `to/destination` only if nothing has that destination name, and
/// never replaces it. Both names resolve against the open directories. On macOS this is
/// `renameatx_np(RENAME_EXCL)`, on Linux `renameat2(RENAME_NOREPLACE)`; anywhere else it
/// refuses and moves nothing.
pub(crate) fn move_exclusive(
    from: &Dir,
    entry: &str,
    to: &Dir,
    destination: &str,
) -> Result<Moved, AcquisitionError> {
    let (source, target) = (name(entry)?, name(destination)?);
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (from, to, source, target);
        Err(AcquisitionError::Storage)
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        #[cfg(target_os = "macos")]
        let moved = unsafe {
            renameatx_np(
                from.0.as_raw_fd(),
                source.as_ptr(),
                to.0.as_raw_fd(),
                target.as_ptr(),
                RENAME_EXCL,
            )
        };
        #[cfg(target_os = "linux")]
        let moved = unsafe {
            libc::renameat2(
                from.0.as_raw_fd(),
                source.as_ptr(),
                to.0.as_raw_fd(),
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if moved == 0 {
            return Ok(Moved::Moved);
        }
        match errno() {
            Some(libc::ENOENT) => Ok(Moved::SourceMissing),
            Some(libc::EEXIST) => Ok(Moved::DestinationTaken),
            _ => Err(AcquisitionError::Storage),
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Runs right after this thread creates a directory, as another process could.
    pub(crate) static AFTER_CREATE: std::cell::RefCell<Option<Box<dyn FnMut(&str)>>> =
        std::cell::RefCell::new(None);
    /// Device and inode of every directory synced on this thread.
    pub(crate) static SYNCED: std::cell::RefCell<Vec<(u64, u64)>> = const { std::cell::RefCell::new(Vec::new()) };
}
