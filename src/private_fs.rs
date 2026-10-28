use std::{
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, PermissionsExt},
    },
    path::{Component, Path},
};
const ERROR: &str = "Private account directory unavailable or unsafe";

/// Creates only inside checked, non-shared parents. Existing permissions are never repaired.
pub(crate) fn prepare(path: &Path) -> Result<File, String> {
    if !path.is_absolute() || path == Path::new("/") {
        return Err(ERROR.into());
    }
    let mut directory = File::open("/").map_err(|_| ERROR)?;
    for component in path.components() {
        let Component::Normal(name) = component else {
            if component == Component::RootDir {
                continue;
            }
            return Err(ERROR.into());
        };
        trusted_parent(&directory)?;
        let name = std::ffi::CString::new(name.as_encoded_bytes()).map_err(|_| ERROR)?;
        let open = || unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY
                    | libc::O_DIRECTORY
                    | libc::O_NOFOLLOW
                    | libc::O_CLOEXEC
                    | libc::O_NONBLOCK,
            )
        };
        let mut fd = open();
        let mut created = false;
        if fd < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            if directory.metadata().map_err(|_| ERROR)?.uid() != unsafe { libc::geteuid() } {
                return Err(ERROR.into());
            }
            if unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                return Err(ERROR.into());
            }
            directory.sync_all().map_err(|_| ERROR)?;
            fd = open();
            created = true;
        }
        if fd < 0 {
            return Err(ERROR.into());
        }
        directory = unsafe { File::from_raw_fd(fd) };
        if created {
            private(&directory)?;
        }
    }
    private(&directory)?;
    Ok(directory)
}
fn trusted_parent(file: &File) -> Result<(), String> {
    let m = file.metadata().map_err(|_| ERROR)?;
    if !m.is_dir()
        || (m.uid() != 0 && m.uid() != unsafe { libc::geteuid() })
        || m.permissions().mode() & 0o022 != 0
    {
        return Err(ERROR.into());
    }
    no_granting_acl(file)
}
fn private(file: &File) -> Result<(), String> {
    let m = file.metadata().map_err(|_| ERROR)?;
    if !m.is_dir()
        || m.uid() != unsafe { libc::geteuid() }
        || m.permissions().mode() & 0o7777 != 0o700
    {
        return Err(ERROR.into());
    }
    no_granting_acl(file)
}

#[cfg(target_os = "macos")]
pub(crate) fn no_granting_acl(file: &File) -> Result<(), String> {
    use std::ffi::{c_int, c_void};
    unsafe extern "C" {
        fn acl_get_fd_np(fd: c_int, kind: c_int) -> *mut c_void;
        fn acl_valid(acl: *mut c_void) -> c_int;
        fn acl_get_entry(acl: *mut c_void, which: c_int, entry: *mut *mut c_void) -> c_int;
        fn acl_get_tag_type(entry: *mut c_void, tag: *mut c_int) -> c_int;
        fn acl_free(object: *mut c_void) -> c_int;
    }
    let acl = unsafe { acl_get_fd_np(file.as_raw_fd(), 0x100) };
    if acl.is_null() {
        return if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            Ok(())
        } else {
            Err(ERROR.into())
        };
    }
    let result = (|| {
        if unsafe { acl_valid(acl) } != 0 {
            return Err(ERROR.into());
        }
        let mut entry = std::ptr::null_mut();
        let mut which = 0;
        loop {
            if unsafe { acl_get_entry(acl, which, &mut entry) } != 0 {
                // macOS returns EINVAL at the end, including for an empty valid ACL.
                return if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINVAL)
                    && unsafe { acl_valid(acl) } == 0
                {
                    Ok(())
                } else {
                    Err(ERROR.into())
                };
            }
            let mut tag = 0;
            if unsafe { acl_get_tag_type(entry, &mut tag) } != 0 || tag != 2 {
                return Err(ERROR.into());
            }
            which = -1;
        }
    })();
    unsafe { acl_free(acl) };
    result
}
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn no_granting_acl(_: &File) -> Result<(), String> {
    Ok(())
}
