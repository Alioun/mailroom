use super::{Document, Snapshot, digest, encode, validate};
#[cfg(unix)]
use crate::private_fs::no_granting_acl;
#[cfg(unix)]
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const ERROR: &str = "Private ledger unavailable or unsafe. Retain files and restore the installed private directory; no paths can be selected by a request";
pub struct InstalledLedger {
    directory: PathBuf,
    #[cfg(unix)]
    identity: (u64, u64),
}
pub struct LockedLedger<'a> {
    installed: &'a InstalledLedger,
    directory: File,
    lock: File,
}
impl Drop for LockedLedger<'_> {
    fn drop(&mut self) {
        let _ = self.lock.unlock();
    }
}
impl InstalledLedger {
    // Installed paths come from the launcher, never an HTTP request.
    pub fn install(directory: PathBuf) -> Result<Self, String> {
        #[cfg(not(unix))]
        {
            let _ = directory;
            return Err("No private ledger adapter on this platform".into());
        }
        #[cfg(unix)]
        {
            if !directory.is_absolute()
                || directory.components().any(|c| {
                    matches!(
                        c,
                        std::path::Component::ParentDir | std::path::Component::CurDir
                    )
                })
            {
                return Err(ERROR.into());
            }
            let parent = directory
                .parent()
                .ok_or(ERROR)?
                .canonicalize()
                .map_err(|_| ERROR)?;
            let parent_fd = open_directory_path(&parent)?;
            let m = parent_fd.metadata().map_err(|_| ERROR)?;
            if m.uid() != unsafe { libc::geteuid() } || m.permissions().mode() & 0o022 != 0 {
                return Err(ERROR.into());
            }
            no_granting_acl(&parent_fd)?;
            let name = directory
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or(ERROR)?;
            Self::install_in(parent, parent_fd, name, true)
        }
    }
    pub(crate) fn install_under(account_data: &Path) -> Result<Self, String> {
        Self::under(account_data, true)
    }
    /// Pins the restore target without creating an owner ledger to be overwritten.
    pub(crate) fn recovery_under(account_data: &Path) -> Result<Self, String> {
        Self::under(account_data, false)
    }
    fn under(account_data: &Path, initialize: bool) -> Result<Self, String> {
        #[cfg(not(unix))]
        {
            let _ = account_data;
            Err(ERROR.into())
        }
        #[cfg(unix)]
        {
            if fs::symlink_metadata(account_data)
                .map_err(|_| ERROR)?
                .file_type()
                .is_symlink()
            {
                return Err(ERROR.into());
            }
            let parent = account_data.canonicalize().map_err(|_| ERROR)?;
            let directory = open_directory_path(&parent)?;
            private_directory(&directory)?;
            Self::install_in(parent, directory, "ledger", initialize)
        }
    }
    #[cfg(unix)]
    fn install_in(
        parent: PathBuf,
        parent_fd: File,
        name: &str,
        initialize: bool,
    ) -> Result<Self, String> {
        let text = std::ffi::CString::new(name).map_err(|_| ERROR)?;
        if unsafe { libc::mkdirat(parent_fd.as_raw_fd(), text.as_ptr(), 0o700) } != 0
            && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST)
        {
            return Err(ERROR.into());
        }
        let dir = open_at(&parent_fd, name, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
        private_directory(&dir)?;
        parent_fd.sync_all().map_err(|_| ERROR)?;
        let m = dir.metadata().map_err(|_| ERROR)?;
        let installed = Self {
            directory: parent.join(name),
            identity: (m.dev(), m.ino()),
        };
        installed.open_directory()?;
        let existed = installed
            .directory
            .join("addresses.lock")
            .try_exists()
            .map_err(|_| ERROR)?;
        let lock = open_at(&dir, "addresses.lock", libc::O_RDWR | libc::O_CREAT, 0o600)?;
        private_regular(&lock)?;
        lock.try_lock()
            .map_err(|_| "Private ledger is in use by another account or process")?;
        let guard = LockedLedger {
            installed: &installed,
            directory: dir,
            lock,
        };
        if !existed && initialize {
            let bytes = encode(&Document::default())?;
            match open_at(
                &guard.directory,
                "addresses.yaml",
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                0o600,
            ) {
                Ok(mut f) => {
                    private_regular(&f)?;
                    f.write_all(&bytes)
                        .and_then(|_| f.sync_all())
                        .map_err(|_| ERROR)?;
                    guard.directory.sync_all().map_err(|_| ERROR)?;
                }
                Err(_) => {
                    guard.read()?;
                }
            }
        } else if guard.raw()?.is_some() {
            guard.read()?;
        }
        drop(guard);
        Ok(installed)
    }
    #[cfg(unix)]
    fn open_directory(&self) -> Result<File, String> {
        let f = open_directory_path(&self.directory)?;
        private_directory(&f)?;
        let m = f.metadata().map_err(|_| ERROR)?;
        if (m.dev(), m.ino()) != self.identity {
            return Err(ERROR.into());
        }
        Ok(f)
    }
    pub fn lock(&self) -> Result<LockedLedger<'_>, String> {
        #[cfg(not(unix))]
        {
            Err("No private ledger adapter on this platform".into())
        }
        #[cfg(unix)]
        {
            let directory = self.open_directory()?;
            let lock = open_at(&directory, "addresses.lock", libc::O_RDWR, 0)?;
            private_regular(&lock)?;
            lock.try_lock()
                .map_err(|_| "Private ledger is in use by another account or process")?;
            let guard = LockedLedger {
                installed: self,
                directory,
                lock,
            };
            guard.check_identity()?;
            Ok(guard)
        }
    }
}
#[cfg(unix)]
fn open_directory_path(path: &Path) -> Result<File, String> {
    if !path.is_absolute() {
        return Err(ERROR.into());
    }
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open("/")
        .map_err(|_| ERROR)?;
    for component in path.components() {
        match component {
            std::path::Component::RootDir => {}
            std::path::Component::Normal(name) => {
                directory = open_at(
                    &directory,
                    name.to_str().ok_or(ERROR)?,
                    libc::O_RDONLY | libc::O_DIRECTORY,
                    0,
                )?;
            }
            _ => return Err(ERROR.into()),
        }
    }
    Ok(directory)
}
#[cfg(unix)]
fn private_directory(file: &File) -> Result<(), String> {
    let m = file.metadata().map_err(|_| ERROR)?;
    if !m.is_dir()
        || m.uid() != unsafe { libc::geteuid() }
        || m.permissions().mode() & 0o7777 != 0o700
    {
        return Err(ERROR.into());
    }
    no_granting_acl(file)
}
#[cfg(unix)]
fn open_at(directory: &File, name: &str, flags: i32, mode: u32) -> Result<File, String> {
    let name = std::ffi::CString::new(name).map_err(|_| ERROR)?;
    // The directory descriptor pins the installed boundary even if its pathname is replaced.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            mode,
        )
    };
    if fd < 0 {
        return Err(ERROR.into());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
#[cfg(unix)]
fn private_regular(file: &File) -> Result<(), String> {
    let m = file.metadata().map_err(|_| ERROR)?;
    if !m.is_file()
        || m.nlink() != 1
        || m.uid() != unsafe { libc::geteuid() }
        || m.permissions().mode() & 0o7777 != 0o600
    {
        return Err(ERROR.into());
    }
    no_granting_acl(file)
}
impl LockedLedger<'_> {
    #[cfg(unix)]
    fn check_identity(&self) -> Result<(), String> {
        let directory = self.installed.open_directory()?;
        let current = open_at(&directory, "addresses.lock", libc::O_RDWR, 0)?;
        let a = current.metadata().map_err(|_| ERROR)?;
        let b = self.lock.metadata().map_err(|_| ERROR)?;
        if (a.dev(), a.ino()) != (b.dev(), b.ino()) {
            return Err(ERROR.into());
        }
        private_regular(&current)
    }
    pub fn read(&self) -> Result<Snapshot, String> {
        #[cfg(not(unix))]
        {
            Err("No private ledger adapter on this platform".into())
        }
        #[cfg(unix)]
        {
            self.check_identity()?;
            let bytes = self.raw()?.ok_or(ERROR)?;
            let document = decode_recovery_bytes(&bytes)?;
            Ok(Snapshot {
                content_version: digest(&bytes),
                document,
            })
        }
    }
    pub(crate) fn raw(&self) -> Result<Option<Vec<u8>>, String> {
        #[cfg(not(unix))]
        {
            Err(ERROR.into())
        }
        #[cfg(unix)]
        {
            self.check_identity()?;
            let fd = unsafe {
                libc::openat(
                    self.directory.as_raw_fd(),
                    c"addresses.yaml".as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                )
            };
            if fd < 0 {
                return if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
                    Ok(None)
                } else {
                    Err(ERROR.into())
                };
            }
            let file = unsafe { File::from_raw_fd(fd) };
            private_regular(&file)?;
            if file.metadata().map_err(|_| ERROR)?.len() > 1024 * 1024 {
                return Err(ERROR.into());
            }
            let mut bytes = Vec::new();
            file.take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| ERROR)?;
            if bytes.len() > 1024 * 1024 {
                return Err(ERROR.into());
            }
            Ok(Some(bytes))
        }
    }
    pub(crate) fn restore_exact(&self, expected: Option<&str>, bytes: &[u8]) -> Result<(), String> {
        #[cfg(not(unix))]
        {
            let _ = (expected, bytes);
            Err(ERROR.into())
        }
        #[cfg(unix)]
        {
            decode_recovery_bytes(bytes)?;
            let current = self.raw()?;
            if current.as_deref() == Some(bytes) {
                return Ok(());
            }
            if current.as_ref().map(|b| digest(b)).as_deref() != expected || current.is_some() {
                return Err("Current owner ledger differs. No restoration published".into());
            }
            let name = format!(
                ".recovery-ledger-{}-{:032x}.stage",
                digest(bytes),
                rand::random::<u128>()
            );
            let mut staged = open_at(
                &self.directory,
                &name,
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )?;
            private_regular(&staged)?;
            staged
                .write_all(bytes)
                .and_then(|_| staged.sync_all())
                .map_err(|_| ERROR)?;
            if self.raw()?.is_some() {
                return Err("Ledger appeared during recovery".into());
            }
            let from = std::ffi::CString::new(name.as_str()).map_err(|_| ERROR)?;
            let fd = self.directory.as_raw_fd();
            // Exclusive rename preserves the one-link guard across process death.
            #[cfg(target_os = "macos")]
            let result = unsafe {
                libc::renameatx_np(
                    fd,
                    from.as_ptr(),
                    fd,
                    c"addresses.yaml".as_ptr(),
                    libc::RENAME_EXCL,
                )
            };
            #[cfg(target_os = "linux")]
            let result = unsafe {
                libc::renameat2(
                    fd,
                    from.as_ptr(),
                    fd,
                    c"addresses.yaml".as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            let result = -1;
            if result != 0 {
                return Err(ERROR.into());
            }
            self.directory.sync_all().map_err(|_| ERROR)?;
            if self.raw()?.as_deref() != Some(bytes) {
                return Err(ERROR.into());
            }
            Ok(())
        }
    }
    pub fn replace(&self, expected: &str, document: &Document) -> Result<Snapshot, String> {
        #[cfg(not(unix))]
        {
            let _ = (expected, document);
            Err("No private ledger adapter on this platform".into())
        }
        #[cfg(unix)]
        {
            validate(document)?;
            let bytes = encode(document)?;
            if self.read()?.content_version != expected {
                return Err(
                    "Private ledger changed outside this review. Reload and review again".into(),
                );
            }
            let name = format!(".addresses-{:032x}.tmp", rand::random::<u128>());
            let mut staged = open_at(
                &self.directory,
                &name,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )?;
            let result = (|| {
                private_regular(&staged)?;
                staged
                    .write_all(&bytes)
                    .and_then(|_| staged.sync_all())
                    .map_err(|_| ERROR)?;
                if self.read()?.content_version != expected {
                    return Err("Private ledger changed while saving. Original retained".into());
                }
                let from = std::ffi::CString::new(name.as_str()).map_err(|_| ERROR)?;
                let to = c"addresses.yaml";
                let fd = self.directory.as_raw_fd();
                if unsafe { libc::renameat(fd, from.as_ptr(), fd, to.as_ptr()) } != 0 {
                    return Err(ERROR.into());
                }
                self.directory.sync_all().map_err(|_| ERROR)?;
                self.read()
            })();
            if result.is_err() {
                if let Ok(name) = std::ffi::CString::new(name) {
                    unsafe {
                        libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0);
                    }
                }
            }
            result
        }
    }
}

pub(crate) fn decode_recovery_bytes(bytes: &[u8]) -> Result<Document, String> {
    if bytes.len() > 1024 * 1024 {
        return Err(ERROR.into());
    }
    let value: serde_json::Value = serde_yaml_ng::from_slice(bytes)
        .map_err(|_| "Unsupported private ledger YAML. Original file retained")?;
    let document: Document = serde_json::from_value(value)
        .map_err(|_| "Unsupported private ledger model. Original file retained")?;
    validate(&document)?;
    Ok(document)
}
