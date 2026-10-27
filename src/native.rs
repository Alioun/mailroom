//! Launches a pinned macOS helper and exchanges one request over private pipes.
//!
//! macOS cannot execute an already-open descriptor (`/dev/fd/N` exec returns EACCES), so the
//! kernel's own code identity of the started process is the check. The helper is started
//! suspended, its kernel-reported cdhash and code-signing flags are compared with the value
//! pinned at build time, and it runs only on a match. Replacing the file at any point before
//! the spawn produces a process with a different cdhash, which is killed before it executes.
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

/// Identity a helper must have, fixed when the main binary is built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pin {
    /// cdhash of the ad-hoc signed, hardened-runtime arm64 helper.
    AdHocCdHash([u8; 20]),
    /// Slot for a later Developer ID designated requirement. This build does not verify it.
    DeveloperIdRequirement(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Helper {
    pub path: PathBuf,
    pub pin: Pin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaunchError {
    NotPinned,
    UnsupportedPin,
    UntrustedFile,
    Spawn,
    IdentityMismatch,
    Channel,
    Timeout,
    Oversized,
    Failed,
}
impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotPinned => "No helper identity was pinned into this build",
            Self::UnsupportedPin => {
                "Developer ID helper verification is not implemented in this build"
            }
            Self::UntrustedFile => {
                "Helper file location, type, owner or permissions are not trusted"
            }
            Self::Spawn => "Helper could not be started",
            Self::IdentityMismatch => "Started helper does not have the pinned code identity",
            Self::Channel => "Helper channel failed",
            Self::Timeout => "Helper did not answer in time",
            Self::Oversized => "Helper answer exceeds its limit",
            Self::Failed => "Helper refused or failed",
        })
    }
}

fn parse_cdhash(hex: &str) -> Option<[u8; 20]> {
    let bytes = data_encoding::HEXLOWER_PERMISSIVE
        .decode(hex.as_bytes())
        .ok()?;
    bytes.try_into().ok()
}

fn pinned(
    path: Option<&'static str>,
    cdhash: Option<&'static str>,
    requirement: Option<&'static str>,
) -> Result<Helper, LaunchError> {
    let path = PathBuf::from(path.ok_or(LaunchError::NotPinned)?);
    if !path.is_absolute() {
        return Err(LaunchError::NotPinned);
    }
    let pin = match (cdhash, requirement) {
        (Some(hash), None) => Pin::AdHocCdHash(parse_cdhash(hash).ok_or(LaunchError::NotPinned)?),
        (None, Some(requirement)) if !requirement.is_empty() => {
            Pin::DeveloperIdRequirement(requirement)
        }
        _ => return Err(LaunchError::NotPinned),
    };
    Ok(Helper { path, pin })
}

/// The confirmation helper pinned into this build. Set by the owner wizard at build time.
pub(crate) fn installed() -> Result<Helper, LaunchError> {
    pinned(
        option_env!("MAILROOM_CONFIRM_HELPER"),
        option_env!("MAILROOM_CONFIRM_HELPER_CDHASH"),
        option_env!("MAILROOM_CONFIRM_HELPER_REQUIREMENT"),
    )
}

/// Sets the core-file limit to zero, soft and hard, so no crash can write a secret to disk.
pub(crate) fn disable_core_dumps() -> bool {
    let zero = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let set = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &zero) };
    set == 0 && core_dumps_disabled()
}
pub(crate) fn core_dumps_disabled() -> bool {
    let mut limit = libc::rlimit {
        rlim_cur: 1,
        rlim_max: 1,
    };
    let read = unsafe { libc::getrlimit(libc::RLIMIT_CORE, &mut limit) };
    read == 0 && limit.rlim_cur == 0 && limit.rlim_max == 0
}

/// The kernel's cdhash for this process, in lowercase hex.
#[cfg(target_os = "macos")]
pub(crate) fn own_cdhash() -> Option<String> {
    macos::cdhash(unsafe { libc::getpid() }).map(|h| data_encoding::HEXLOWER.encode(&h))
}
/// Hardened runtime blocks debugger attachment and injected libraries, which would
/// otherwise let a same-user process read keys out of this binary.
#[cfg(target_os = "macos")]
pub(crate) fn own_process_hardened() -> bool {
    macos::flags(unsafe { libc::getpid() })
        .is_some_and(|f| f & macos::REQUIRED_FLAGS == macos::REQUIRED_FLAGS)
}

pub(crate) const REQUEST_LIMIT: usize = 65536;

/// Starts the pinned helper, verifies the running image, writes `input` and returns its answer.
pub(crate) fn exchange(
    helper: &Helper,
    input: &[u8],
    limit: usize,
    deadline: Duration,
) -> Result<Zeroizing<Vec<u8>>, LaunchError> {
    exchange_with(helper, input, limit, deadline, || {})
}

#[cfg(not(target_os = "macos"))]
fn exchange_with(
    _: &Helper,
    _: &[u8],
    _: usize,
    _: Duration,
    _: impl FnOnce(),
) -> Result<Zeroizing<Vec<u8>>, LaunchError> {
    Err(LaunchError::UnsupportedPin)
}

#[cfg(target_os = "macos")]
fn exchange_with(
    helper: &Helper,
    input: &[u8],
    limit: usize,
    deadline: Duration,
    between_check_and_spawn: impl FnOnce(),
) -> Result<Zeroizing<Vec<u8>>, LaunchError> {
    let Pin::AdHocCdHash(expected) = &helper.pin else {
        return Err(LaunchError::UnsupportedPin);
    };
    if input.len() > REQUEST_LIMIT {
        return Err(LaunchError::Oversized);
    }
    check_file(&helper.path)?;
    between_check_and_spawn();
    let child = macos::Child::spawn_suspended(&helper.path)?;
    child.require_identity(expected)?;
    child.run(input, limit, Instant::now() + deadline)
}

/// Rejects symlinks, non-regular files and files or directories other local users could replace.
#[cfg(target_os = "macos")]
fn check_file(path: &Path) -> Result<(), LaunchError> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let uid = unsafe { libc::getuid() };
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| LaunchError::UntrustedFile)?;
    let meta = file.metadata().map_err(|_| LaunchError::UntrustedFile)?;
    let parent = path
        .parent()
        .and_then(|p| std::fs::symlink_metadata(p).ok())
        .ok_or(LaunchError::UntrustedFile)?;
    let trusted = |owner: u32, mode: u32| (owner == uid || owner == 0) && mode & 0o022 == 0;
    if !meta.file_type().is_file()
        || meta.mode() & 0o111 == 0
        || !trusted(meta.uid(), meta.mode())
        || !parent.file_type().is_dir()
        || !trusted(parent.uid(), parent.mode())
    {
        return Err(LaunchError::UntrustedFile);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod macos {
    use super::LaunchError;
    use std::{
        ffi::CString,
        io::{Read, Write},
        os::fd::{FromRawFd, OwnedFd},
        path::Path,
        time::Instant,
    };
    use subtle::ConstantTimeEq;
    use zeroize::Zeroizing;

    const CS_OPS_STATUS: u32 = 0;
    const CS_OPS_CDHASH: u32 = 5;
    const CS_VALID: u32 = 0x0000_0001;
    const CS_HARD: u32 = 0x0000_0100;
    const CS_KILL: u32 = 0x0000_0200;
    const CS_RUNTIME: u32 = 0x0001_0000;
    pub(super) const REQUIRED_FLAGS: u32 = CS_VALID | CS_HARD | CS_KILL | CS_RUNTIME;

    unsafe extern "C" {
        fn csops(
            pid: libc::pid_t,
            ops: u32,
            useraddr: *mut libc::c_void,
            usersize: libc::size_t,
        ) -> libc::c_int;
    }

    pub(super) fn flags(pid: libc::pid_t) -> Option<u32> {
        let mut flags = 0u32;
        (unsafe { csops(pid, CS_OPS_STATUS, (&mut flags as *mut u32).cast(), 4) } == 0)
            .then_some(flags)
    }
    pub(super) fn cdhash(pid: libc::pid_t) -> Option<[u8; 20]> {
        let mut cdhash = [0u8; 20];
        (unsafe { csops(pid, CS_OPS_CDHASH, cdhash.as_mut_ptr().cast(), 20) } == 0)
            .then_some(cdhash)
    }

    fn pipe() -> Result<(OwnedFd, OwnedFd), LaunchError> {
        let mut fds = [0; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(LaunchError::Spawn);
        }
        let ends = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        for fd in fds {
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        Ok(ends)
    }

    /// A started helper. Dropping it before a clean exit kills and reaps the process.
    pub(super) struct Child {
        pid: libc::pid_t,
        stdin: Option<std::fs::File>,
        stdout: std::fs::File,
        reaped: bool,
    }

    impl Child {
        pub(super) fn spawn_suspended(path: &Path) -> Result<Self, LaunchError> {
            use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};
            let program =
                CString::new(path.as_os_str().as_bytes()).map_err(|_| LaunchError::Spawn)?;
            let name = c"mailroom-helper";
            let argv = [name.as_ptr() as *mut libc::c_char, std::ptr::null_mut()];
            let envp = [std::ptr::null_mut::<libc::c_char>()];
            let (child_in, parent_in) = pipe()?;
            let (parent_out, child_out) = pipe()?;
            let mut pid = 0;
            let status = unsafe {
                let mut actions = std::mem::zeroed::<libc::posix_spawn_file_actions_t>();
                let mut attributes = std::mem::zeroed::<libc::posix_spawnattr_t>();
                libc::posix_spawn_file_actions_init(&mut actions);
                libc::posix_spawnattr_init(&mut attributes);
                libc::posix_spawn_file_actions_adddup2(&mut actions, child_in.as_raw_fd(), 0);
                libc::posix_spawn_file_actions_adddup2(&mut actions, child_out.as_raw_fd(), 1);
                libc::posix_spawn_file_actions_addopen(
                    &mut actions,
                    2,
                    c"/dev/null".as_ptr(),
                    libc::O_WRONLY,
                    0,
                );
                libc::posix_spawnattr_setflags(
                    &mut attributes,
                    (libc::POSIX_SPAWN_START_SUSPENDED | libc::POSIX_SPAWN_CLOEXEC_DEFAULT)
                        as libc::c_short,
                );
                let status = libc::posix_spawn(
                    &mut pid,
                    program.as_ptr(),
                    &actions,
                    &attributes,
                    argv.as_ptr(),
                    envp.as_ptr(),
                );
                libc::posix_spawn_file_actions_destroy(&mut actions);
                libc::posix_spawnattr_destroy(&mut attributes);
                status
            };
            drop((child_in, child_out));
            if status != 0 {
                return Err(LaunchError::Spawn);
            }
            Ok(Self {
                pid,
                stdin: Some(std::fs::File::from(parent_in)),
                stdout: std::fs::File::from(parent_out),
                reaped: false,
            })
        }

        /// Reads the identity the kernel attached to the started image, before it has run.
        pub(super) fn require_identity(&self, expected: &[u8; 20]) -> Result<(), LaunchError> {
            match (flags(self.pid), cdhash(self.pid)) {
                (Some(flags), Some(cdhash))
                    if flags & REQUIRED_FLAGS == REQUIRED_FLAGS
                        && bool::from(cdhash.ct_eq(expected)) =>
                {
                    Ok(())
                }
                _ => Err(LaunchError::IdentityMismatch),
            }
        }

        pub(super) fn run(
            mut self,
            input: &[u8],
            limit: usize,
            deadline: Instant,
        ) -> Result<Zeroizing<Vec<u8>>, LaunchError> {
            use std::os::fd::AsRawFd;
            if unsafe { libc::kill(self.pid, libc::SIGCONT) } != 0 {
                return Err(LaunchError::Spawn);
            }
            let mut stdin = self.stdin.take().ok_or(LaunchError::Channel)?;
            stdin.write_all(input).map_err(|_| LaunchError::Channel)?;
            drop(stdin);
            let mut output = Zeroizing::new(Vec::with_capacity(limit + 1));
            let mut chunk = Zeroizing::new([0u8; 4096]);
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(LaunchError::Timeout);
                }
                let mut poll = libc::pollfd {
                    fd: self.stdout.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                let timeout = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
                match unsafe { libc::poll(&mut poll, 1, timeout) } {
                    0 => return Err(LaunchError::Timeout),
                    n if n < 0 => return Err(LaunchError::Channel),
                    _ => {}
                }
                let read = self
                    .stdout
                    .read(&mut chunk[..])
                    .map_err(|_| LaunchError::Channel)?;
                if read == 0 {
                    break;
                }
                if output.len() + read > limit {
                    return Err(LaunchError::Oversized);
                }
                output.extend_from_slice(&chunk[..read]);
            }
            let mut status = 0;
            loop {
                match unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) } {
                    0 if Instant::now() < deadline => {
                        std::thread::sleep(std::time::Duration::from_millis(5))
                    }
                    0 => return Err(LaunchError::Timeout),
                    pid if pid == self.pid => break,
                    _ => return Err(LaunchError::Channel),
                }
            }
            self.reaped = true;
            if !libc::WIFEXITED(status) || libc::WEXITSTATUS(status) != 0 {
                return Err(LaunchError::Failed);
            }
            Ok(output)
        }
    }

    impl Drop for Child {
        fn drop(&mut self) {
            if !self.reaped {
                unsafe {
                    libc::kill(self.pid, libc::SIGKILL);
                    libc::waitpid(self.pid, std::ptr::null_mut(), 0);
                }
            }
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) mod tests;
