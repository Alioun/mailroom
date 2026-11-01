use super::{Proposal, REQUEST_LIMIT, SUMMARY_LIMIT, digest};
use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path},
};

const ERROR: &str = "agent_exchange_unavailable";

pub(super) struct Exchange {
    reviewed: File,
    proposals: File,
}

fn directory(parent: &File, name: &str, create: bool) -> Result<File, &'static str> {
    let name = CString::new(name).map_err(|_| ERROR)?;
    if create {
        let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
        if result < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(ERROR);
        }
        parent.sync_all().map_err(|_| ERROR)?;
    }
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(ERROR);
    }
    checked_directory(unsafe { File::from_raw_fd(fd) })
}

fn checked_directory(file: File) -> Result<File, &'static str> {
    let metadata = file.metadata().map_err(|_| ERROR)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(ERROR);
    }
    Ok(file)
}

fn root(path: &Path) -> Result<File, &'static str> {
    if !path.is_absolute()
        || path == Path::new("/")
        || path.components().any(|c| c == Component::ParentDir)
    {
        return Err(ERROR);
    }
    checked_directory(
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| ERROR)?,
    )
}

fn read(parent: &File, name: &str, limit: usize) -> Result<Vec<u8>, &'static str> {
    let name = CString::new(name).map_err(|_| ERROR)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(ERROR);
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata().map_err(|_| ERROR)?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
        || metadata.len() > limit as u64
    {
        return Err(ERROR);
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ERROR)?;
    if bytes.len() > limit {
        return Err(ERROR);
    }
    Ok(bytes)
}

/// Creates `name` with `bytes`. Returns false, writing nothing, when it exists.
fn create(parent: &File, name: &str, bytes: &[u8]) -> Result<bool, &'static str> {
    let name = CString::new(name).map_err(|_| ERROR)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return match std::io::Error::last_os_error().kind() {
            std::io::ErrorKind::AlreadyExists => Ok(false),
            _ => Err(ERROR),
        };
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(bytes).map_err(|_| ERROR)?;
    file.sync_all().map_err(|_| ERROR)?;
    parent.sync_all().map_err(|_| ERROR)?;
    Ok(true)
}

fn write_once(parent: &File, name: &str, bytes: &[u8]) -> Result<(), &'static str> {
    if create(parent, name, bytes)? || read(parent, name, bytes.len())? == bytes {
        return Ok(());
    }
    Err(ERROR)
}

/// A file `init` installs. `superseded` holds every earlier packaged version.
/// A file still equal to one of them was never changed by the user, so `init`
/// replaces it. Any other content belongs to the user and is refused.
struct Packaged {
    name: &'static str,
    current: &'static [u8],
    superseded: &'static [&'static [u8]],
}

const AGENTS: Packaged = Packaged {
    name: "AGENTS.md",
    current: include_bytes!("../../agent/AGENTS.md"),
    superseded: &[include_bytes!("../../agent/superseded/021643c/AGENTS.md")],
};
const SKILL: Packaged = Packaged {
    name: "SKILL.md",
    current: include_bytes!("../../agent/skills/mailroom-assistant/SKILL.md"),
    superseded: &[include_bytes!(
        "../../agent/superseded/021643c/skills/mailroom-assistant/SKILL.md"
    )],
};

fn install(parent: &File, packaged: &Packaged) -> Result<(), &'static str> {
    if create(parent, packaged.name, packaged.current)? {
        return Ok(());
    }
    let limit = packaged
        .superseded
        .iter()
        .map(|b| b.len())
        .chain([packaged.current.len()])
        .max()
        .unwrap_or(0);
    let existing = read(parent, packaged.name, limit)?;
    if existing == packaged.current {
        return Ok(());
    }
    if !packaged.superseded.contains(&existing.as_slice()) {
        return Err(ERROR);
    }
    let staging = format!(".{}.update", packaged.name);
    let staging_c = CString::new(staging.as_str()).map_err(|_| ERROR)?;
    let name_c = CString::new(packaged.name).map_err(|_| ERROR)?;
    if unsafe { libc::unlinkat(parent.as_raw_fd(), staging_c.as_ptr(), 0) } < 0
        && std::io::Error::last_os_error().kind() != std::io::ErrorKind::NotFound
    {
        return Err(ERROR);
    }
    if !create(parent, &staging, packaged.current)? {
        return Err(ERROR);
    }
    if unsafe {
        libc::renameat(
            parent.as_raw_fd(),
            staging_c.as_ptr(),
            parent.as_raw_fd(),
            name_c.as_ptr(),
        )
    } < 0
    {
        return Err(ERROR);
    }
    parent.sync_all().map_err(|_| ERROR)
}

impl Exchange {
    pub(super) fn initialize(path: &Path) -> Result<Self, &'static str> {
        if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
            return Err(ERROR);
        }
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err(ERROR),
        }
        let root = root(path)?;
        let reviewed = directory(&root, "reviewed", true)?;
        let proposals = directory(&root, "proposals", true)?;
        let skills = directory(&root, "skills", true)?;
        let skill = directory(&skills, "mailroom-assistant", true)?;
        install(&root, &AGENTS)?;
        install(&skill, &SKILL)?;
        Ok(Self {
            reviewed,
            proposals,
        })
    }

    pub(super) fn open(path: &Path) -> Result<Self, &'static str> {
        let root = root(path)?;
        Ok(Self {
            reviewed: directory(&root, "reviewed", false)?,
            proposals: directory(&root, "proposals", false)?,
        })
    }

    pub(super) fn summary(&self) -> Result<String, &'static str> {
        let bytes = read(&self.reviewed, "census-summary.md", SUMMARY_LIMIT)?;
        let summary = String::from_utf8(bytes).map_err(|_| ERROR)?;
        if !summary.starts_with("# Census summary\n") || summary.contains('\0') {
            return Err("invalid_summary_export");
        }
        Ok(summary)
    }

    pub(super) fn submit(&self, proposal: Proposal) -> Result<String, &'static str> {
        proposal.validate(&self.summary()?)?;
        let bytes = serde_json::to_vec_pretty(&proposal).map_err(|_| "invalid_proposal")?;
        if bytes.len() > REQUEST_LIMIT {
            return Err("proposal_too_large");
        }
        let id = digest(&bytes);
        write_once(&self.proposals, &format!("{id}.json"), &bytes)?;
        Ok(id)
    }
}
