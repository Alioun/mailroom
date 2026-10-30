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

fn write_once(parent: &File, name: &str, bytes: &[u8]) -> Result<(), &'static str> {
    let name_c = CString::new(name).map_err(|_| ERROR)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name_c.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        if std::io::Error::last_os_error().kind() == std::io::ErrorKind::AlreadyExists
            && read(parent, name, bytes.len())? == bytes
        {
            return Ok(());
        }
        return Err(ERROR);
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(bytes).map_err(|_| ERROR)?;
    file.sync_all().map_err(|_| ERROR)?;
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
        write_once(&root, "AGENTS.md", include_bytes!("../../agent/AGENTS.md"))?;
        write_once(
            &skill,
            "SKILL.md",
            include_bytes!("../../agent/skills/mailroom-assistant/SKILL.md"),
        )?;
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
