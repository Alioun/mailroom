//! The local web vault: every credential of the browser setup, sealed in one file under a
//! key derived from the master password. It is the only owner of that key and of the
//! decrypted entries, and it is the credential backend of every web account.
//!
//! `vault.mrv`, all integers big-endian:
//!
//! | bytes  | field                                                                |
//! |--------|----------------------------------------------------------------------|
//! | 0..8   | `MRVAULT1`                                                           |
//! | 8..10  | version 1: Argon2id v0x13, m=65536 KiB, t=3, p=4, 32-byte key        |
//! | 10..26 | salt, set once at creation                                           |
//! | 26..34 | generation, 1 up to 2^32                                             |
//! | 34..46 | nonce, 12 random bytes, new on every write                           |
//! | 46..50 | ciphertext length, at most 1 MiB + 16, equal to file length minus 50 |
//! | 50..   | AES-256-GCM ciphertext and tag, with bytes 0..50 as AAD              |
//!
//! The plaintext is a counted list of at most 1024 entries in strictly increasing reference
//! order: a `u16` count, then per entry a `u8` reference length, the reference
//! (`[a-z0-9-]{1,128}`), a `u16` value length (at most 4096) and the value.
use super::{CredentialBackend, SecretValue};
use crate::{acquisition::AcquisitionError, file_lock::ExclusiveFileLock};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::AeadInPlace};
use rand::{RngCore, rngs::OsRng};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    ffi::{CStr, CString},
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, PermissionsExt},
    },
    path::Path,
    sync::{Mutex, MutexGuard},
};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

const FILE: &CStr = c"vault.mrv";
const LOCK: &CStr = c"vault.lock";
const STAGE: &str = ".vault.stage-";
const MAGIC: &[u8; 8] = b"MRVAULT1";
const VERSION: u16 = 1;
const HEADER: usize = 50;
const TAG: usize = 16;
const MAX_CIPHERTEXT: usize = 1024 * 1024 + TAG;
const MAX_FILE: u64 = (HEADER + MAX_CIPHERTEXT) as u64;
const MAX_GENERATION: u64 = 1 << 32;
const MAX_ENTRIES: usize = 1024;
const MAX_REFERENCE: usize = 128;
const MAX_VALUE: usize = 4096;
pub(crate) const MIN_PASSWORD_CHARS: usize = 12;
const MAX_PASSWORD_BYTES: usize = 1024;

type Entries = BTreeMap<String, Zeroizing<Vec<u8>>>;

/// What the browser may know about the vault.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VaultStatus {
    Absent,
    Locked,
    Unlocked,
    Unavailable,
}

/// Why an existing vault file cannot be used. Mailroom never repairs or replaces it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Problem {
    UnsafeFile,
    Damaged,
    UnsupportedVersion,
}

/// Fixed outcomes with no payload, so no input or secret byte can reach a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VaultError {
    Exists,
    Absent,
    Busy,
    InUse,
    UnlockFailed,
    Unavailable,
    WeakPassword,
    Mismatch,
    Unsafe,
    Storage,
}
impl VaultError {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Exists => "exists",
            Self::Absent => "absent",
            Self::Busy => "busy",
            Self::InUse => "in_use",
            Self::UnlockFailed => "unlock_failed",
            Self::Unavailable => "unavailable",
            Self::WeakPassword => "weak_password",
            Self::Mismatch => "mismatch",
            Self::Unsafe => "unsafe",
            Self::Storage => "storage",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Identity {
    device: u64,
    inode: u64,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl Identity {
    fn of(metadata: &std::fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

struct Open {
    key: Zeroizing<[u8; 32]>,
    salt: [u8; 16],
    generation: u64,
    identity: Identity,
    entries: Entries,
}
enum State {
    Absent,
    Locked,
    Unavailable(Problem),
    Unlocked(Box<Open>),
}

/// The Argon2id cost. Format version 1 fixes it; tests use a cheap one so that hundreds of
/// derivations stay fast, and such a vault never opens under the standard cost.
#[derive(Clone, Copy)]
enum Cost {
    Standard,
    #[cfg(test)]
    Test,
}

pub(crate) struct Vault {
    directory: File,
    /// Held for the whole process, so a second server refuses before it serves anything.
    _lease: ExclusiveFileLock,
    /// One create or unlock at a time, and so one key derivation at a time.
    transition: Mutex<()>,
    state: Mutex<State>,
    cost: Cost,
    #[cfg(test)]
    derivations: std::sync::atomic::AtomicUsize,
}

impl Vault {
    /// Prepares the private root, takes the lease and inspects the file without deriving a key.
    pub(crate) fn open(root: &Path) -> Result<Self, VaultError> {
        Self::open_with(root, Cost::Standard)
    }
    /// A vault with a cheap key derivation, which never opens a standard vault.
    #[cfg(test)]
    pub(crate) fn open_for_tests(root: &Path) -> Result<Self, VaultError> {
        Self::open_with(root, Cost::Test)
    }
    fn open_with(root: &Path, cost: Cost) -> Result<Self, VaultError> {
        let directory = crate::private_fs::prepare(root).map_err(|_| VaultError::Unsafe)?;
        let lease = lease(&directory)?;
        remove_stages(&directory, root)?;
        let state = match read(&directory) {
            Ok(None) => State::Absent,
            Ok(Some((bytes, _))) => match parse(&bytes) {
                Ok(_) => State::Locked,
                Err(problem) => State::Unavailable(problem),
            },
            Err(problem) => State::Unavailable(problem),
        };
        Ok(Self {
            directory,
            _lease: lease,
            transition: Mutex::new(()),
            state: Mutex::new(state),
            cost,
            #[cfg(test)]
            derivations: std::sync::atomic::AtomicUsize::new(0),
        })
    }
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    pub(crate) fn status(&self) -> VaultStatus {
        match &*self.state() {
            State::Absent => VaultStatus::Absent,
            State::Locked => VaultStatus::Locked,
            State::Unavailable(_) => VaultStatus::Unavailable,
            State::Unlocked(_) => VaultStatus::Unlocked,
        }
    }
    pub(crate) fn problem(&self) -> Option<Problem> {
        match &*self.state() {
            State::Unavailable(problem) => Some(*problem),
            _ => None,
        }
    }
    pub(crate) fn unlocked(&self) -> bool {
        matches!(&*self.state(), State::Unlocked(_))
    }

    /// Creates the vault, empty, and leaves it unlocked.
    pub(crate) fn create(
        &self,
        password: &SecretValue,
        confirmation: &SecretValue,
    ) -> Result<(), VaultError> {
        let _transition = self.transition.try_lock().map_err(|_| VaultError::Busy)?;
        match &*self.state() {
            State::Absent => {}
            State::Unavailable(_) => return Err(VaultError::Unavailable),
            State::Locked | State::Unlocked(_) => return Err(VaultError::Exists),
        }
        new_password(password, confirmation)?;
        let mut salt = [0; 16];
        OsRng.fill_bytes(&mut salt);
        let key = self.derive(password.bytes(), &salt)?;
        let entries = Entries::new();
        let sealed = seal(&key, &salt, 1, &entries).map_err(|_| VaultError::Storage)?;
        let identity = match write(&self.directory, &sealed, Placement::New) {
            Ok(identity) => identity,
            Err(Written::No(exists)) => {
                return Err(if exists {
                    VaultError::Exists
                } else {
                    VaultError::Storage
                });
            }
            Err(Written::Uncertain) => {
                *self.state() = State::Locked;
                return Err(VaultError::Storage);
            }
        };
        *self.state() = State::Unlocked(Box::new(Open {
            key,
            salt,
            generation: 1,
            identity,
            entries,
        }));
        Ok(())
    }

    /// Unlocks with the master password. A wrong password and a changed file look the same.
    pub(crate) fn unlock(&self, password: &SecretValue) -> Result<(), VaultError> {
        let _transition = self.transition.try_lock().map_err(|_| VaultError::Busy)?;
        match &*self.state() {
            State::Locked => {}
            State::Unlocked(_) => return Ok(()),
            State::Absent => return Err(VaultError::Absent),
            State::Unavailable(_) => return Err(VaultError::Unavailable),
        }
        if password.bytes().is_empty() || password.bytes().len() > MAX_PASSWORD_BYTES {
            return Err(VaultError::UnlockFailed);
        }
        let (bytes, identity) = match read(&self.directory) {
            Ok(Some(found)) => found,
            Ok(None) => {
                *self.state() = State::Absent;
                return Err(VaultError::Absent);
            }
            Err(problem) => {
                *self.state() = State::Unavailable(problem);
                return Err(VaultError::Unavailable);
            }
        };
        let header = match parse(&bytes) {
            Ok(header) => header,
            Err(problem) => {
                *self.state() = State::Unavailable(problem);
                return Err(VaultError::Unavailable);
            }
        };
        let key = self.derive(password.bytes(), &header.salt)?;
        let plaintext = unseal(&key, &bytes).map_err(|_| VaultError::UnlockFailed)?;
        let Ok(entries) = decode(&plaintext) else {
            *self.state() = State::Unavailable(Problem::Damaged);
            return Err(VaultError::Unavailable);
        };
        *self.state() = State::Unlocked(Box::new(Open {
            key,
            salt: header.salt,
            generation: header.generation,
            identity,
            entries,
        }));
        Ok(())
    }

    fn derive(&self, password: &[u8], salt: &[u8; 16]) -> Result<Zeroizing<[u8; 32]>, VaultError> {
        #[cfg(test)]
        self.derivations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (memory, passes, lanes) = match self.cost {
            Cost::Standard => (65536, 3, 4),
            #[cfg(test)]
            Cost::Test => (64, 1, 1),
        };
        let params = argon2::Params::new(memory, passes, lanes, Some(32))
            .map_err(|_| VaultError::Storage)?;
        let mut blocks = Zeroizing::new(vec![argon2::Block::default(); params.block_count()]);
        let mut key = Zeroizing::new([0; 32]);
        argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
            .hash_password_into_with_memory(password, salt, key.as_mut(), blocks.as_mut_slice())
            .map_err(|_| VaultError::Storage)?;
        Ok(key)
    }

    /// Applies `edit` to a copy of the entries and commits it before the open state changes.
    /// `edit` returns whether anything changed; nothing is written when it did not.
    fn change(
        &self,
        edit: impl FnOnce(&mut Entries) -> Result<bool, AcquisitionError>,
    ) -> Result<bool, AcquisitionError> {
        let mut state = self.state();
        let State::Unlocked(open) = &mut *state else {
            return Err(AcquisitionError::VaultLocked);
        };
        let mut next = open.entries.clone();
        if !edit(&mut next)? {
            return Ok(false);
        }
        if next.len() > MAX_ENTRIES {
            return Err(AcquisitionError::Storage);
        }
        // Another writer replaced the file: forget everything and make the owner unlock again.
        if current(&self.directory) != Some((open.identity, open.generation, open.salt)) {
            *state = State::Locked;
            return Err(AcquisitionError::VaultLocked);
        }
        let generation = open.generation + 1;
        if generation > MAX_GENERATION {
            return Err(AcquisitionError::Storage);
        }
        let sealed = seal(&open.key, &open.salt, generation, &next)?;
        match write(&self.directory, &sealed, Placement::Replace) {
            Ok(identity) => {
                open.entries = next;
                open.generation = generation;
                open.identity = identity;
                Ok(true)
            }
            Err(Written::No(_)) => Err(AcquisitionError::Storage),
            Err(Written::Uncertain) => {
                *state = State::Locked;
                Err(AcquisitionError::Storage)
            }
        }
    }
}

impl CredentialBackend for Vault {
    fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
        valid_reference(reference)?;
        match &*self.state() {
            State::Unlocked(open) => open
                .entries
                .get(reference)
                .map(|value| SecretValue::new(value.to_vec()))
                .ok_or(AcquisitionError::CredentialMissing),
            _ => Err(AcquisitionError::VaultLocked),
        }
    }
    fn put(&self, reference: &str, value: &SecretValue) -> Result<(), AcquisitionError> {
        valid_reference(reference)?;
        if value.bytes().len() > MAX_VALUE {
            return Err(AcquisitionError::InvalidInput);
        }
        self.change(|entries| {
            entries.insert(reference.to_owned(), Zeroizing::new(value.bytes().to_vec()));
            Ok(true)
        })
        .map(|_| ())
    }
    fn delete(&self, reference: &str) -> Result<(), AcquisitionError> {
        valid_reference(reference)?;
        self.change(|entries| Ok(entries.remove(reference).is_some()))
            .map(|_| ())
    }
    /// Checks and removes under one hold of the state, so the deleted value is the checked one.
    fn delete_owned(
        &self,
        reference: &str,
        owned: &dyn Fn(&[u8]) -> bool,
    ) -> Result<bool, AcquisitionError> {
        valid_reference(reference)?;
        self.change(|entries| {
            if entries.get(reference).is_some_and(|value| owned(value)) {
                entries.remove(reference);
                Ok(true)
            } else {
                Ok(false)
            }
        })
    }
}

fn valid_reference(reference: &str) -> Result<(), AcquisitionError> {
    if (1..=MAX_REFERENCE).contains(&reference.len())
        && reference
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        Ok(())
    } else {
        Err(AcquisitionError::InvalidInput)
    }
}

fn new_password(password: &SecretValue, confirmation: &SecretValue) -> Result<(), VaultError> {
    let text = std::str::from_utf8(password.bytes()).map_err(|_| VaultError::WeakPassword)?;
    if text.chars().count() < MIN_PASSWORD_CHARS
        || text.len() > MAX_PASSWORD_BYTES
        || text.chars().any(char::is_control)
    {
        return Err(VaultError::WeakPassword);
    }
    if !bool::from(password.bytes().ct_eq(confirmation.bytes())) {
        return Err(VaultError::Mismatch);
    }
    Ok(())
}

struct Header {
    salt: [u8; 16],
    generation: u64,
}
/// Every bound is checked here, before any key derivation.
fn parse(bytes: &[u8]) -> Result<Header, Problem> {
    if bytes.len() < HEADER + TAG || bytes.len() as u64 > MAX_FILE || &bytes[..8] != MAGIC {
        return Err(Problem::Damaged);
    }
    if u16::from_be_bytes([bytes[8], bytes[9]]) != VERSION {
        return Err(Problem::UnsupportedVersion);
    }
    let generation = u64::from_be_bytes(bytes[26..34].try_into().expect("8 bytes"));
    let length = u32::from_be_bytes(bytes[46..50].try_into().expect("4 bytes")) as usize;
    if !(1..=MAX_GENERATION).contains(&generation)
        || length > MAX_CIPHERTEXT
        || length != bytes.len() - HEADER
    {
        return Err(Problem::Damaged);
    }
    Ok(Header {
        salt: bytes[10..26].try_into().expect("16 bytes"),
        generation,
    })
}

fn seal(
    key: &[u8; 32],
    salt: &[u8; 16],
    generation: u64,
    entries: &Entries,
) -> Result<Vec<u8>, AcquisitionError> {
    let mut buffer = encode(entries)?;
    let mut nonce = [0; 12];
    OsRng.fill_bytes(&mut nonce);
    let mut header = [0; HEADER];
    header[..8].copy_from_slice(MAGIC);
    header[8..10].copy_from_slice(&VERSION.to_be_bytes());
    header[10..26].copy_from_slice(salt);
    header[26..34].copy_from_slice(&generation.to_be_bytes());
    header[34..46].copy_from_slice(&nonce);
    header[46..50].copy_from_slice(&((buffer.len() + TAG) as u32).to_be_bytes());
    Aes256Gcm::new_from_slice(key)
        .map_err(|_| AcquisitionError::Storage)?
        .encrypt_in_place(Nonce::from_slice(&nonce), &header, &mut *buffer)
        .map_err(|_| AcquisitionError::Storage)?;
    let mut sealed = Vec::with_capacity(HEADER + buffer.len());
    sealed.extend_from_slice(&header);
    sealed.extend_from_slice(&buffer);
    Ok(sealed)
}

fn unseal(key: &[u8; 32], bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>, ()> {
    // Decrypted in an erasing buffer, so a failed tag check leaves no plaintext behind.
    let mut buffer = Zeroizing::new(bytes[HEADER..].to_vec());
    Aes256Gcm::new_from_slice(key)
        .map_err(|_| ())?
        .decrypt_in_place(
            Nonce::from_slice(&bytes[34..46]),
            &bytes[..HEADER],
            &mut *buffer,
        )
        .map_err(|_| ())?;
    Ok(buffer)
}

fn encode(entries: &Entries) -> Result<Zeroizing<Vec<u8>>, AcquisitionError> {
    let size = 2 + entries
        .iter()
        .map(|(reference, value)| 3 + reference.len() + value.len())
        .sum::<usize>();
    if entries.len() > MAX_ENTRIES || size + TAG > MAX_CIPHERTEXT {
        return Err(AcquisitionError::Storage);
    }
    // Sized for the tag too, so neither encoding nor sealing reallocates and strands a copy.
    let mut out = Zeroizing::new(Vec::with_capacity(size + TAG));
    out.extend_from_slice(&(entries.len() as u16).to_be_bytes());
    for (reference, value) in entries {
        out.push(reference.len() as u8);
        out.extend_from_slice(reference.as_bytes());
        out.extend_from_slice(&(value.len() as u16).to_be_bytes());
        out.extend_from_slice(value);
    }
    Ok(out)
}

fn decode(plaintext: &[u8]) -> Result<Entries, ()> {
    fn take<'a>(rest: &mut &'a [u8], n: usize) -> Result<&'a [u8], ()> {
        if rest.len() < n {
            return Err(());
        }
        let (head, tail) = rest.split_at(n);
        *rest = tail;
        Ok(head)
    }
    fn length(rest: &mut &[u8]) -> Result<usize, ()> {
        let bytes = take(rest, 2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]) as usize)
    }
    let mut rest = plaintext;
    let count = length(&mut rest)?;
    if count > MAX_ENTRIES {
        return Err(());
    }
    let mut entries = Entries::new();
    for _ in 0..count {
        let size = take(&mut rest, 1)?[0] as usize;
        let reference = std::str::from_utf8(take(&mut rest, size)?).map_err(|_| ())?;
        valid_reference(reference).map_err(|_| ())?;
        if entries
            .last_key_value()
            .is_some_and(|(last, _)| last.as_str() >= reference)
        {
            return Err(());
        }
        let size = length(&mut rest)?;
        if size > MAX_VALUE {
            return Err(());
        }
        let value = Zeroizing::new(take(&mut rest, size)?.to_vec());
        entries.insert(reference.to_owned(), value);
    }
    if !rest.is_empty() {
        return Err(());
    }
    Ok(entries)
}

fn open_at(directory: &File, name: &CStr, flags: libc::c_int) -> std::io::Result<File> {
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600 as libc::c_uint,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// A regular file owned by this user, readable by nobody else, with one link and no ACL.
fn private_file(file: &File) -> Option<std::fs::Metadata> {
    let metadata = file.metadata().ok()?;
    (metadata.is_file()
        && metadata.uid() == unsafe { libc::geteuid() }
        && metadata.permissions().mode() & 0o7777 == 0o600
        && metadata.nlink() == 1
        && crate::private_fs::no_granting_acl(file).is_ok())
    .then_some(metadata)
}

fn lease(directory: &File) -> Result<ExclusiveFileLock, VaultError> {
    let file =
        open_at(directory, LOCK, libc::O_RDWR | libc::O_CREAT).map_err(|_| VaultError::Unsafe)?;
    private_file(&file).ok_or(VaultError::Unsafe)?;
    ExclusiveFileLock::try_lock(file).map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => VaultError::InUse,
        std::fs::TryLockError::Error(_) => VaultError::Storage,
    })
}

/// Removes staged writes a killed process left. Only the lease holder runs it.
fn remove_stages(directory: &File, root: &Path) -> Result<(), VaultError> {
    for entry in std::fs::read_dir(root).map_err(|_| VaultError::Storage)? {
        let name = entry.map_err(|_| VaultError::Storage)?.file_name();
        if name.to_str().is_some_and(|n| n.starts_with(STAGE)) {
            let name = CString::new(name.as_encoded_bytes()).map_err(|_| VaultError::Storage)?;
            if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                return Err(VaultError::Storage);
            }
        }
    }
    directory.sync_all().map_err(|_| VaultError::Storage)
}

/// The vault file, if there is one, refused without reading when unsafe or oversized.
fn read(directory: &File) -> Result<Option<(Vec<u8>, Identity)>, Problem> {
    let mut file = match open_at(directory, FILE, libc::O_RDONLY | libc::O_NONBLOCK) {
        Ok(file) => file,
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => return Ok(None),
        Err(_) => return Err(Problem::UnsafeFile),
    };
    let metadata = private_file(&file).ok_or(Problem::UnsafeFile)?;
    if metadata.len() > MAX_FILE {
        return Err(Problem::Damaged);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(MAX_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Problem::Damaged)?;
    if bytes.len() as u64 != metadata.len() {
        return Err(Problem::Damaged);
    }
    Ok(Some((bytes, Identity::of(&metadata))))
}

/// The identity, generation and salt of the file now on disk.
fn current(directory: &File) -> Option<(Identity, u64, [u8; 16])> {
    let mut file = open_at(directory, FILE, libc::O_RDONLY | libc::O_NONBLOCK).ok()?;
    let metadata = private_file(&file)?;
    let mut header = [0; HEADER];
    file.read_exact(&mut header).ok()?;
    if &header[..8] != MAGIC {
        return None;
    }
    Some((
        Identity::of(&metadata),
        u64::from_be_bytes(header[26..34].try_into().ok()?),
        header[10..26].try_into().ok()?,
    ))
}

enum Placement {
    New,
    Replace,
}
enum Written {
    /// Nothing changed on disk; `true` when the vault already existed.
    No(bool),
    /// The new file may be in place, so memory can no longer be trusted.
    Uncertain,
}
fn write(directory: &File, bytes: &[u8], placement: Placement) -> Result<Identity, Written> {
    let name = CString::new(format!("{STAGE}{}", super::lifecycle::nonce()))
        .map_err(|_| Written::No(false))?;
    let mut file = open_at(
        directory,
        &name,
        libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
    )
    .map_err(|_| Written::No(false))?;
    let discard = || unsafe {
        libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0);
    };
    let staged = private_file(&file).is_some()
        && file.write_all(bytes).is_ok()
        && file.sync_all().is_ok()
        && super::lifecycle::boundary("vault_staged").is_ok();
    if !staged {
        discard();
        return Err(Written::No(false));
    }
    let fd = directory.as_raw_fd();
    let renamed = match placement {
        Placement::Replace => unsafe { libc::renameat(fd, name.as_ptr(), fd, FILE.as_ptr()) },
        #[cfg(target_os = "macos")]
        Placement::New => unsafe {
            libc::renameatx_np(fd, name.as_ptr(), fd, FILE.as_ptr(), libc::RENAME_EXCL)
        },
        #[cfg(target_os = "linux")]
        Placement::New => unsafe {
            libc::renameat2(fd, name.as_ptr(), fd, FILE.as_ptr(), libc::RENAME_NOREPLACE)
        },
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        Placement::New => -1,
    };
    if renamed != 0 {
        let exists = std::io::Error::last_os_error().raw_os_error() == Some(libc::EEXIST);
        discard();
        return Err(Written::No(exists));
    }
    if super::lifecycle::boundary("vault_renamed").is_err() || directory.sync_all().is_err() {
        return Err(Written::Uncertain);
    }
    file.metadata()
        .map(|metadata| Identity::of(&metadata))
        .map_err(|_| Written::Uncertain)
}

#[cfg(test)]
mod tests;
