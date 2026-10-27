//! macOS credentials held by the hardened, pinned mailroom binary itself.
//!
//! No helper process touches the Keychain, so no other program can ask one for key bytes.
//! Mailbox passwords trust no application: macOS asks a person at every read, and every
//! read first checks that the item's access lists still trust nobody. Census and identity
//! keys trust exactly this binary's cdhash; an item trusting anything else is refused.
//! Changing an access list needs a person (`SecKeychainItemSetAccess` waits for the
//! system dialog). Any same-user process can delete items, which access lists cannot stop.
use super::{AcquisitionError, CredentialBackend, SecretValue};
use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::{CFType, CFTypeRef, OSStatus, TCFType},
    boolean::CFBoolean,
    data::CFData,
    dictionary::CFDictionary,
    string::{CFString, CFStringRef},
};
use std::{
    ffi::{CString, c_char, c_int, c_void},
    path::{Path, PathBuf},
};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

type SecKeychainItemRef = *mut c_void;
type SecAccessRef = *mut c_void;
type SecAclRef = *mut c_void;
type SecTrustedApplicationRef = *mut c_void;
type SecRequirementRef = *mut c_void;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecClass: CFStringRef;
    static kSecClassGenericPassword: CFStringRef;
    static kSecAttrService: CFStringRef;
    static kSecAttrAccount: CFStringRef;
    static kSecAttrLabel: CFStringRef;
    static kSecAttrAccess: CFStringRef;
    static kSecValueData: CFStringRef;
    static kSecReturnData: CFStringRef;
    static kSecReturnRef: CFStringRef;
    static kSecReturnAttributes: CFStringRef;
    static kSecMatchLimit: CFStringRef;
    static kSecMatchLimitOne: CFStringRef;
    static kSecMatchLimitAll: CFStringRef;
    static kSecMatchSearchList: CFStringRef;
    static kSecMatchItemList: CFStringRef;
    static kSecUseKeychain: CFStringRef;
    fn SecKeychainOpen(path: *const c_char, keychain: *mut *mut c_void) -> OSStatus;
    fn SecKeychainSetUserInteractionAllowed(allowed: u8) -> OSStatus;
    fn SecKeychainCopySearchList(list: *mut CFArrayRef) -> OSStatus;
    fn SecKeychainGetPath(keychain: *mut c_void, length: *mut u32, path: *mut c_char) -> OSStatus;
    fn SecItemAdd(attributes: CFTypeRef, result: *mut CFTypeRef) -> OSStatus;
    fn SecItemCopyMatching(query: CFTypeRef, result: *mut CFTypeRef) -> OSStatus;
    fn SecKeychainItemDelete(item: SecKeychainItemRef) -> OSStatus;
    fn SecKeychainItemCopyAccess(item: SecKeychainItemRef, access: *mut SecAccessRef) -> OSStatus;
    fn SecAccessCreate(
        label: CFStringRef,
        trusted: CFArrayRef,
        access: *mut SecAccessRef,
    ) -> OSStatus;
    fn SecAccessCopyACLList(access: SecAccessRef, list: *mut CFArrayRef) -> OSStatus;
    fn SecACLCopyAuthorizations(acl: SecAclRef) -> CFArrayRef;
    fn SecACLCopyContents(
        acl: SecAclRef,
        applications: *mut CFArrayRef,
        description: *mut CFStringRef,
        selector: *mut u16,
    ) -> OSStatus;
    fn SecTrustedApplicationCreateFromPath(
        path: *const c_char,
        application: *mut SecTrustedApplicationRef,
    ) -> OSStatus;
    fn SecTrustedApplicationCopyRequirement(
        application: SecTrustedApplicationRef,
        requirement: *mut SecRequirementRef,
    ) -> OSStatus;
    fn SecRequirementCopyString(
        requirement: SecRequirementRef,
        flags: u32,
        text: *mut CFStringRef,
    ) -> OSStatus;
}

unsafe extern "C" {
    fn acl_get_link_np(path: *const c_char, kind: c_int) -> *mut c_void;
    fn acl_get_entry(acl: *mut c_void, which: c_int, entry: *mut *mut c_void) -> c_int;
    fn acl_get_tag_type(entry: *mut c_void, tag: *mut c_int) -> c_int;
    fn acl_free(object: *mut c_void) -> c_int;
}
const ACL_TYPE_EXTENDED: c_int = 0x100;
const ACL_FIRST_ENTRY: c_int = 0;
const ACL_NEXT_ENTRY: c_int = -1;
const ACL_EXTENDED_DENY: c_int = 2;

pub(crate) const SERVICE: &str = "mailroom-protected-v1";
pub(crate) const LEGACY_SERVICE: &str = "mailroom";

/// Fixed result codes. No OS text or item data ever leaves this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Ok,
    Missing,
    Exists,
    Denied,
    /// The access lists differ from what this module creates.
    Changed,
    /// A key item trusts exactly one other binary, such as an earlier mailroom build.
    OtherBinary,
    Failed,
}
impl Status {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Missing => "missing",
            Self::Exists => "exists",
            Self::Denied => "denied",
            Self::Changed => "changed",
            Self::OtherBinary => "other_binary",
            Self::Failed => "failed",
        }
    }
    fn of(status: OSStatus) -> Self {
        match status {
            0 => Self::Ok,
            -25300 => Self::Missing,
            -25299 => Self::Exists,
            -25293 | -25308 | -128 => Self::Denied,
            _ => Self::Failed,
        }
    }
}
impl From<Status> for AcquisitionError {
    fn from(status: Status) -> Self {
        match status {
            Status::Missing => AcquisitionError::CredentialMissing,
            _ => AcquisitionError::Credentials,
        }
    }
}

/// How an item may be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    /// No application is trusted; macOS asks a person at every read.
    EveryAccess,
    /// Exactly this binary is trusted.
    PinnedSelf,
}
impl Class {
    /// Local census and identity keys are read at every store open. Everything else,
    /// mailbox passwords and unknown references included, asks a person at every read.
    pub(crate) fn of(reference: &str) -> Self {
        let generation = reference
            .rsplit_once("-key-g")
            .is_some_and(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if reference.ends_with("-census-key") || reference.ends_with("-identity") || generation {
            Self::PinnedSelf
        } else {
            Self::EveryAccess
        }
    }
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::EveryAccess => "every_access",
            Self::PinnedSelf => "pinned_self",
        }
    }
}

pub(crate) fn valid_reference(reference: &str) -> bool {
    (1..=128).contains(&reference.len())
        && reference
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// What an item's access lists allow, read from the lists themselves.
#[derive(Debug, PartialEq, Eq)]
enum Observed {
    EveryAccess,
    Pinned(String),
    Changed,
}

/// One access-list entry as macOS reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub authorizations: Vec<String>,
    /// None means any application may use the entry without a prompt.
    pub trusted: Option<Vec<String>>,
    pub selector: u16,
}

/// The prompt selector `SecAccessCreate` gives every entry. Any other selector means the
/// prompt policy was edited, so the item is refused.
pub(crate) const CREATED_SELECTOR: u16 = 0;

/// The policy check, separate from the Security calls so every shape can be tested.
/// Encryption-only entries are ignored. Change-ACL entries must trust nobody. Every other
/// entry must carry the created selector, trust a listed set and agree with the others.
fn classify(entries: &[Entry]) -> Observed {
    let mut reading: Option<&Vec<String>> = None;
    for entry in entries {
        if entry
            .authorizations
            .iter()
            .all(|a| a == "ACLAuthorizationEncrypt")
        {
            continue;
        }
        let Some(trusted) = &entry.trusted else {
            return Observed::Changed;
        };
        if entry.selector != CREATED_SELECTOR {
            return Observed::Changed;
        }
        if entry
            .authorizations
            .iter()
            .all(|a| a == "ACLAuthorizationChangeACL")
        {
            if !trusted.is_empty() {
                return Observed::Changed;
            }
            continue;
        }
        match reading {
            None => reading = Some(trusted),
            Some(previous) if previous == trusted => {}
            Some(_) => return Observed::Changed,
        }
    }
    match reading {
        Some(trusted) if trusted.is_empty() => Observed::EveryAccess,
        Some(trusted) if trusted.len() == 1 => Observed::Pinned(trusted[0].clone()),
        _ => Observed::Changed,
    }
}

struct Owned(*mut c_void);
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { core_foundation::base::CFRelease(self.0 as CFTypeRef) };
        }
    }
}

fn key(name: CFStringRef) -> CFString {
    unsafe { CFString::wrap_under_get_rule(name) }
}

fn requirement_of(application: SecTrustedApplicationRef) -> Option<String> {
    let mut requirement = std::ptr::null_mut();
    if unsafe { SecTrustedApplicationCopyRequirement(application, &mut requirement) } != 0
        || requirement.is_null()
    {
        return None;
    }
    let requirement = Owned(requirement);
    let mut text = std::ptr::null();
    if unsafe { SecRequirementCopyString(requirement.0, 0, &mut text) } != 0 || text.is_null() {
        return None;
    }
    Some(unsafe { CFString::wrap_under_create_rule(text) }.to_string())
}

/// This binary as a trusted application. The requirement macOS records for it must be
/// exactly the running image's cdhash.
fn own_application() -> Result<(Owned, String), Status> {
    let mut application = std::ptr::null_mut();
    if unsafe { SecTrustedApplicationCreateFromPath(std::ptr::null(), &mut application) } != 0
        || application.is_null()
    {
        return Err(Status::Failed);
    }
    let application = Owned(application);
    let requirement = requirement_of(application.0).ok_or(Status::Failed)?;
    let own = crate::native::own_cdhash().ok_or(Status::Failed)?;
    if requirement != format!("cdhash H\"{own}\"") {
        return Err(Status::Failed);
    }
    Ok((application, requirement))
}

pub(crate) struct Keychain {
    /// None is the user's default keychain, used only by the hardened production binary.
    explicit: Option<CFType>,
}
unsafe impl Send for Keychain {}
unsafe impl Sync for Keychain {}

/// The production backend: only a hardened binary with core dumps disabled may use it.
pub(crate) fn installed() -> Result<Keychain, AcquisitionError> {
    if !crate::native::own_process_hardened() || !crate::native::core_dumps_disabled() {
        return Err(AcquisitionError::Credentials);
    }
    if unsafe { SecKeychainSetUserInteractionAllowed(1) } != 0 {
        return Err(AcquisitionError::Credentials);
    }
    Ok(Keychain { explicit: None })
}

fn keychain_path(keychain: *mut c_void) -> Result<PathBuf, Status> {
    let mut buffer = [0 as c_char; 1024];
    let mut length = buffer.len() as u32;
    if unsafe { SecKeychainGetPath(keychain, &mut length, buffer.as_mut_ptr()) } != 0 {
        return Err(Status::Failed);
    }
    let path = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) };
    Ok(PathBuf::from(path.to_str().map_err(|_| Status::Failed)?))
}

/// Every keychain on the user's search list. Any failure refuses rather than returning less.
fn search_list_paths() -> Result<Vec<PathBuf>, Status> {
    let mut list = std::ptr::null();
    if unsafe { SecKeychainCopySearchList(&mut list) } != 0 || list.is_null() {
        return Err(Status::Failed);
    }
    let list: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(list) };
    let mut paths = Vec::new();
    for keychain in list.iter() {
        let path = keychain_path(keychain.as_CFTypeRef() as *mut c_void)?;
        paths.push(path.canonicalize().map_err(|_| Status::Failed)?);
    }
    if paths.is_empty() {
        return Err(Status::Failed);
    }
    Ok(paths)
}

/// The account's home directory from the user database, not from the environment.
fn account_home() -> Result<PathBuf, Status> {
    let mut record = unsafe { std::mem::zeroed::<libc::passwd>() };
    let mut buffer = vec![0 as c_char; 16384];
    let mut found = std::ptr::null_mut();
    let status = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            &mut record,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut found,
        )
    };
    if status != 0 || found.is_null() || record.pw_dir.is_null() {
        return Err(Status::Failed);
    }
    let home = unsafe { std::ffi::CStr::from_ptr(record.pw_dir) };
    Ok(PathBuf::from(home.to_str().map_err(|_| Status::Failed)?))
}

/// Whether `path` itself carries an ACL entry that grants anything. Deny entries only take
/// rights away, like the one macOS puts on home folders. An unreadable ACL counts as a grant.
fn acl_grants(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(text) = CString::new(path.as_os_str().as_bytes()) else {
        return true;
    };
    let acl = unsafe { acl_get_link_np(text.as_ptr(), ACL_TYPE_EXTENDED) };
    if acl.is_null() {
        return std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOENT);
    }
    let mut grants = false;
    let mut entry = std::ptr::null_mut();
    let mut which = ACL_FIRST_ENTRY;
    while unsafe { acl_get_entry(acl, which, &mut entry) } == 0 {
        let mut tag = 0;
        if unsafe { acl_get_tag_type(entry, &mut tag) } != 0 || tag != ACL_EXTENDED_DENY {
            grants = true;
            break;
        }
        which = ACL_NEXT_ENTRY;
    }
    unsafe { acl_free(acl) };
    grants
}

/// Opens `path` without following a final symlink and reads its metadata from the open
/// descriptor, so the identity checked is the file that was opened.
fn opened_metadata(path: &Path) -> Result<std::fs::Metadata, Status> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .and_then(|file| file.metadata())
        .map_err(|_| Status::Failed)
}

/// An explicit keychain file for tests. It must be an existing regular file with one link,
/// owned by this user in a directory nobody else can write. Every directory above it must
/// belong to this user or root, without group or other write and without granting ACLs.
/// It must lie outside the system and user keychain folders and off the search list, and
/// the keychain Security opens must report the same file. Security opens keychains by
/// path, at open and again at later use, so these checks bind the path, not a handle; a
/// process running as this user can still swap the file afterwards. Any doubt refuses, and
/// nothing ever falls back to the default keychain.
pub(crate) fn explicit(path: &Path) -> Result<Keychain, Status> {
    use std::os::unix::fs::MetadataExt;
    if !path.is_absolute() {
        return Err(Status::Failed);
    }
    let canonical = path.canonicalize().map_err(|_| Status::Failed)?;
    let meta = opened_metadata(&canonical)?;
    let parent = std::fs::symlink_metadata(canonical.parent().ok_or(Status::Failed)?)
        .map_err(|_| Status::Failed)?;
    let uid = unsafe { libc::getuid() };
    let private = |m: &std::fs::Metadata| m.uid() == uid && m.mode() & 0o022 == 0;
    let guarded = canonical.ancestors().skip(1).all(|directory| {
        std::fs::symlink_metadata(directory)
            .is_ok_and(|m| m.is_dir() && (m.uid() == uid || m.uid() == 0) && m.mode() & 0o022 == 0)
            && !acl_grants(directory)
    });
    let home = account_home()?.canonicalize().map_err(|_| Status::Failed)?;
    let protected = [
        PathBuf::from("/Library"),
        PathBuf::from("/System"),
        PathBuf::from("/private/var/db"),
        home.join("Library/Keychains"),
    ]
    .into_iter()
    .any(|root| canonical.starts_with(root));
    let listed = search_list_paths()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || !private(&meta)
        || !private(&parent)
        || !guarded
        || acl_grants(&canonical)
        || protected
        || listed.contains(&canonical)
    {
        return Err(Status::Failed);
    }
    let text =
        CString::new(canonical.to_str().ok_or(Status::Failed)?).map_err(|_| Status::Failed)?;
    let mut keychain = std::ptr::null_mut();
    if unsafe { SecKeychainOpen(text.as_ptr(), &mut keychain) } != 0 || keychain.is_null() {
        return Err(Status::Failed);
    }
    let keychain = unsafe { CFType::wrap_under_create_rule(keychain as CFTypeRef) };
    let opened = keychain_path(keychain.as_CFTypeRef() as *mut c_void)?;
    let again = opened_metadata(&opened)?;
    if opened.canonicalize().map_err(|_| Status::Failed)? != canonical
        || (again.dev(), again.ino(), again.nlink()) != (meta.dev(), meta.ino(), 1)
        || search_list_paths()?.contains(&canonical)
    {
        return Err(Status::Failed);
    }
    if unsafe { SecKeychainSetUserInteractionAllowed(0) } != 0 {
        return Err(Status::Failed);
    }
    Ok(Keychain {
        explicit: Some(keychain),
    })
}

fn equal(a: &[u8], b: &[u8]) -> bool {
    bool::from(a.ct_eq(b))
}

impl Keychain {
    fn query(&self, service: &str, reference: Option<&str>) -> Vec<(CFString, CFType)> {
        let mut pairs = vec![
            (
                key(unsafe { kSecClass }),
                key(unsafe { kSecClassGenericPassword }).as_CFType(),
            ),
            (
                key(unsafe { kSecAttrService }),
                CFString::new(service).as_CFType(),
            ),
        ];
        if let Some(reference) = reference {
            pairs.push((
                key(unsafe { kSecAttrAccount }),
                CFString::new(reference).as_CFType(),
            ));
        }
        if let Some(keychain) = &self.explicit {
            pairs.push((
                key(unsafe { kSecMatchSearchList }),
                CFArray::from_CFTypes(&[keychain.clone()]).as_CFType(),
            ));
        }
        pairs
    }

    fn copy_matching(pairs: Vec<(CFString, CFType)>) -> (Status, Option<CFType>) {
        let query = CFDictionary::from_CFType_pairs(&pairs);
        let mut result = std::ptr::null();
        let status = Status::of(unsafe { SecItemCopyMatching(query.as_CFTypeRef(), &mut result) });
        let result = (!result.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(result) });
        (status, result)
    }

    fn item(&self, service: &str, reference: &str) -> Result<CFType, Status> {
        let mut pairs = self.query(service, Some(reference));
        pairs.push((
            key(unsafe { kSecReturnRef }),
            CFBoolean::true_value().as_CFType(),
        ));
        pairs.push((
            key(unsafe { kSecMatchLimit }),
            key(unsafe { kSecMatchLimitOne }).as_CFType(),
        ));
        match Self::copy_matching(pairs) {
            (Status::Ok, Some(item)) => Ok(item),
            (Status::Ok, None) => Err(Status::Failed),
            (status, _) => Err(status),
        }
    }

    /// Reads the bytes of exactly this item. Prompts when its access lists require it.
    fn data(&self, item: &CFType) -> Result<Zeroizing<Vec<u8>>, Status> {
        let mut pairs = vec![
            (
                key(unsafe { kSecClass }),
                key(unsafe { kSecClassGenericPassword }).as_CFType(),
            ),
            (
                key(unsafe { kSecMatchItemList }),
                CFArray::from_CFTypes(&[item.clone()]).as_CFType(),
            ),
            (
                key(unsafe { kSecReturnData }),
                CFBoolean::true_value().as_CFType(),
            ),
            (
                key(unsafe { kSecMatchLimit }),
                key(unsafe { kSecMatchLimitOne }).as_CFType(),
            ),
        ];
        if let Some(keychain) = &self.explicit {
            pairs.push((
                key(unsafe { kSecMatchSearchList }),
                CFArray::from_CFTypes(&[keychain.clone()]).as_CFType(),
            ));
        }
        match Self::copy_matching(pairs) {
            (Status::Ok, Some(value)) => {
                let data = value.downcast::<CFData>().ok_or(Status::Failed)?;
                let bytes = Zeroizing::new(data.bytes().to_vec());
                if bytes.is_empty() {
                    return Err(Status::Failed);
                }
                Ok(bytes)
            }
            (Status::Ok, None) => Err(Status::Failed),
            (status, _) => Err(status),
        }
    }

    /// Reads every access-list entry of an item: authorizations, trusted requirements
    /// (None means any application) and prompt selector.
    fn entries(item: &CFType) -> Option<Vec<Entry>> {
        let mut access = std::ptr::null_mut();
        if unsafe {
            SecKeychainItemCopyAccess(item.as_CFTypeRef() as SecKeychainItemRef, &mut access)
        } != 0
            || access.is_null()
        {
            return None;
        }
        let access = Owned(access);
        let mut list = std::ptr::null();
        if unsafe { SecAccessCopyACLList(access.0, &mut list) } != 0 || list.is_null() {
            return None;
        }
        let list: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(list) };
        let mut entries = Vec::new();
        for acl in list.iter() {
            let acl = acl.as_CFTypeRef() as SecAclRef;
            let authorizations = unsafe { SecACLCopyAuthorizations(acl) };
            if authorizations.is_null() {
                return None;
            }
            let authorizations: CFArray<CFString> =
                unsafe { CFArray::wrap_under_create_rule(authorizations) };
            let mut applications = std::ptr::null();
            let mut description = std::ptr::null();
            let mut selector = 0u16;
            if unsafe {
                SecACLCopyContents(acl, &mut applications, &mut description, &mut selector)
            } != 0
            {
                return None;
            }
            if !description.is_null() {
                drop(unsafe { CFString::wrap_under_create_rule(description) });
            }
            let trusted = if applications.is_null() {
                None
            } else {
                let applications: CFArray<CFType> =
                    unsafe { CFArray::wrap_under_create_rule(applications) };
                let mut trusted = Vec::new();
                for application in applications.iter() {
                    trusted.push(requirement_of(
                        application.as_CFTypeRef() as SecTrustedApplicationRef
                    )?);
                }
                Some(trusted)
            };
            entries.push(Entry {
                authorizations: authorizations.iter().map(|a| a.to_string()).collect(),
                trusted,
                selector,
            });
        }
        Some(entries)
    }

    fn observe(item: &CFType) -> Observed {
        Self::entries(item).map_or(Observed::Changed, |entries| classify(&entries))
    }

    /// Checks one item's access lists against the class its reference requires.
    fn check(item: &CFType, class: Class) -> Status {
        match (Self::observe(item), class) {
            (Observed::EveryAccess, Class::EveryAccess) => Status::Ok,
            (Observed::Pinned(requirement), Class::PinnedSelf) => match own_application() {
                Ok((_, own)) if own == requirement => Status::Ok,
                Ok(_) if requirement.starts_with("cdhash H\"") => Status::OtherBinary,
                Ok(_) => Status::Changed,
                Err(status) => status,
            },
            _ => Status::Changed,
        }
    }

    pub(crate) fn status(&self, reference: &str) -> Status {
        self.status_as(reference, Class::of(reference))
    }
    fn status_as(&self, reference: &str, class: Class) -> Status {
        if !valid_reference(reference) {
            return Status::Failed;
        }
        match self.item(SERVICE, reference) {
            Ok(item) => Self::check(&item, class),
            Err(status) => status,
        }
    }

    /// Reads a protected value after confirming its access lists are unchanged.
    pub(crate) fn read(&self, reference: &str) -> Result<Zeroizing<Vec<u8>>, Status> {
        self.read_as(reference, Class::of(reference))
    }
    fn read_as(&self, reference: &str, class: Class) -> Result<Zeroizing<Vec<u8>>, Status> {
        if !valid_reference(reference) {
            return Err(Status::Failed);
        }
        let item = self.item(SERVICE, reference)?;
        match Self::check(&item, class) {
            Status::Ok => self.data(&item),
            status => Err(status),
        }
    }

    /// Creates an item. An existing item is never overwritten.
    pub(crate) fn create(&self, reference: &str, secret: &[u8]) -> Status {
        self.create_as(reference, Class::of(reference), secret)
    }
    fn create_as(&self, reference: &str, class: Class, secret: &[u8]) -> Status {
        if !valid_reference(reference) || secret.is_empty() || secret.len() > 8192 {
            return Status::Failed;
        }
        let own = match class {
            Class::PinnedSelf => match own_application() {
                Ok((application, _)) => Some(application),
                Err(status) => return status,
            },
            Class::EveryAccess => None,
        };
        let trusted: Vec<CFType> = own
            .iter()
            .map(|a| unsafe { CFType::wrap_under_get_rule(a.0 as CFTypeRef) })
            .collect();
        self.add(SERVICE, reference, &CFArray::from_CFTypes(&trusted), secret)
    }

    fn add(
        &self,
        service: &str,
        reference: &str,
        trusted: &CFArray<CFType>,
        secret: &[u8],
    ) -> Status {
        let label = CFString::new(&format!("Mailroom: {reference}"));
        let mut access = std::ptr::null_mut();
        if unsafe {
            SecAccessCreate(
                label.as_concrete_TypeRef(),
                trusted.as_concrete_TypeRef(),
                &mut access,
            )
        } != 0
            || access.is_null()
        {
            return Status::Failed;
        }
        let access = unsafe { CFType::wrap_under_create_rule(access as CFTypeRef) };
        // The CFData borrows the caller's buffer, so this side makes no unwiped copy.
        let data = unsafe {
            CFData::wrap_under_create_rule(core_foundation::data::CFDataCreateWithBytesNoCopy(
                std::ptr::null(),
                secret.as_ptr(),
                secret.len() as isize,
                core_foundation::base::kCFAllocatorNull,
            ))
        };
        let search = key(unsafe { kSecMatchSearchList });
        let mut pairs = self.query(service, Some(reference));
        pairs.retain(|(k, _)| *k != search);
        pairs.push((key(unsafe { kSecAttrLabel }), label.as_CFType()));
        pairs.push((key(unsafe { kSecAttrAccess }), access));
        pairs.push((key(unsafe { kSecValueData }), data.as_CFType()));
        if let Some(keychain) = &self.explicit {
            pairs.push((key(unsafe { kSecUseKeychain }), keychain.clone()));
        }
        let attributes = CFDictionary::from_CFType_pairs(&pairs);
        Status::of(unsafe { SecItemAdd(attributes.as_CFTypeRef(), std::ptr::null_mut()) })
    }

    pub(crate) fn remove(&self, reference: &str) -> Status {
        self.remove_in(SERVICE, reference)
    }
    fn remove_in(&self, service: &str, reference: &str) -> Status {
        if !valid_reference(reference) {
            return Status::Failed;
        }
        match self.item(service, reference) {
            Ok(item) => Status::of(unsafe {
                SecKeychainItemDelete(item.as_CFTypeRef() as SecKeychainItemRef)
            }),
            Err(status) => status,
        }
    }

    /// Lists legacy `keyring` item names from their attributes. Values are never read here.
    pub(crate) fn legacy_references(&self) -> Result<Vec<String>, Status> {
        let mut pairs = self.query(LEGACY_SERVICE, None);
        pairs.push((
            key(unsafe { kSecReturnAttributes }),
            CFBoolean::true_value().as_CFType(),
        ));
        pairs.push((
            key(unsafe { kSecMatchLimit }),
            key(unsafe { kSecMatchLimitAll }).as_CFType(),
        ));
        let rows = match Self::copy_matching(pairs) {
            (Status::Missing, _) => return Ok(vec![]),
            (Status::Ok, Some(rows)) => rows,
            (Status::Ok, None) => return Err(Status::Failed),
            (status, _) => return Err(status),
        };
        let rows = rows.downcast::<CFArray>().ok_or(Status::Failed)?;
        let account = key(unsafe { kSecAttrAccount });
        let mut names: Vec<String> = rows
            .iter()
            .filter_map(|row| {
                let row: CFDictionary<CFString, CFType> =
                    unsafe { CFDictionary::wrap_under_get_rule(*row as _) };
                row.find(&account)
                    .and_then(|v| v.downcast::<CFString>())
                    .map(|s| s.to_string())
            })
            .filter(|n| valid_reference(n))
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    fn legacy_read(&self, reference: &str) -> Result<Zeroizing<Vec<u8>>, Status> {
        if !valid_reference(reference) {
            return Err(Status::Failed);
        }
        self.data(&self.item(LEGACY_SERVICE, reference)?)
    }

    /// Copies a legacy password into a protected item, then proves the protected item holds
    /// it. Keys move only with their whole account, through `gather` and `settle`. Returns
    /// `Ok` when created and `Exists` when an equal item was already there.
    pub(crate) fn migrate(&self, reference: &str) -> Result<Status, Migration> {
        if Class::of(reference) != Class::EveryAccess {
            return Err(Migration::Refused);
        }
        let legacy = self.legacy_read(reference).map_err(Migration::Legacy)?;
        let outcome = match self.status(reference) {
            Status::Missing => match self.create(reference, &legacy) {
                Status::Ok => Status::Ok,
                status => return Err(Migration::Protected(status)),
            },
            Status::Ok => Status::Exists,
            status => return Err(Migration::Protected(status)),
        };
        let protected = self.read(reference).map_err(Migration::Protected)?;
        if equal(&protected, &legacy) {
            Ok(outcome)
        } else {
            Err(Migration::Mismatch)
        }
    }

    /// Deletes a legacy original only when the protected item is correctly protected and
    /// holds the same value.
    pub(crate) fn retire_legacy(&self, reference: &str) -> Result<(), Migration> {
        let protected = self.read(reference).map_err(Migration::Protected)?;
        let legacy = self.legacy_read(reference).map_err(Migration::Legacy)?;
        if !equal(&protected, &legacy) {
            return Err(Migration::Mismatch);
        }
        match self.remove_in(LEGACY_SERVICE, reference) {
            Status::Ok => Ok(()),
            status => Err(Migration::Legacy(status)),
        }
    }

    /// Finds one key's value wherever an earlier build or an interrupted move left it, and
    /// changes nothing. Reading an earlier build's item needs a person. Nothing here says
    /// the value is genuine: anyone can plant an item, so the caller checks the account's
    /// whole set against its saved state before `settle` moves any of it.
    pub(crate) fn gather(&self, reference: &str) -> Result<Gathered, Status> {
        if !valid_reference(reference) || Class::of(reference) != Class::PinnedSelf {
            return Err(Status::Failed);
        }
        let staged_reference = staged(reference);
        let staged = match self.status_as(&staged_reference, Class::PinnedSelf) {
            Status::Ok => Some(self.read_as(&staged_reference, Class::PinnedSelf)?),
            Status::Missing => None,
            status => return Err(status),
        };
        let (origin, value) = match (self.status(reference), staged) {
            (Status::Ok, staged) => {
                let value = self.read(reference)?;
                if staged.as_ref().is_some_and(|copy| !equal(copy, &value)) {
                    return Err(Status::Changed);
                }
                let origin = match staged {
                    Some(_) => Origin::CurrentAndStaged,
                    None => Origin::Current,
                };
                (origin, value)
            }
            (Status::OtherBinary, _) => {
                let item = self.item(SERVICE, reference)?;
                let value = self.data(&item)?;
                (Origin::Earlier(item), value)
            }
            (Status::Missing, Some(value)) => (Origin::Staged, value),
            (Status::Missing, None) => (Origin::Legacy, self.legacy_read(reference)?),
            (status, _) => return Err(status),
        };
        Ok(Gathered {
            reference: reference.to_owned(),
            origin,
            value,
        })
    }

    /// Leaves one verified key as a current item trusting this binary. A move from an
    /// earlier build goes through a staged copy, so a rerun after an interruption at any
    /// step gathers the key again from where it was left.
    pub(crate) fn settle(&self, key: &Gathered) -> Result<(), Status> {
        let staged = staged(&key.reference);
        match &key.origin {
            Origin::Current => Ok(()),
            Origin::CurrentAndStaged => self.removed(&staged),
            Origin::Staged => self.publish(&key.reference, &staged, &key.value),
            Origin::Legacy => self.created(&key.reference, Class::PinnedSelf, &key.value),
            Origin::Earlier(item) => {
                self.removed(&staged)?;
                self.created(&staged, Class::PinnedSelf, &key.value)?;
                match Status::of(unsafe {
                    SecKeychainItemDelete(item.as_CFTypeRef() as SecKeychainItemRef)
                }) {
                    Status::Ok => {}
                    status => return Err(status),
                }
                self.publish(&key.reference, &staged, &key.value)
            }
        }
    }

    fn publish(&self, reference: &str, staged: &str, value: &[u8]) -> Result<(), Status> {
        self.created(reference, Class::PinnedSelf, value)?;
        self.removed(staged)
    }

    /// Creates an item and proves it reads back as `value`.
    fn created(&self, reference: &str, class: Class, value: &[u8]) -> Result<(), Status> {
        match self.create_as(reference, class, value) {
            Status::Ok => {}
            status => return Err(status),
        }
        if equal(&self.read_as(reference, class)?, value) {
            Ok(())
        } else {
            Err(Status::Failed)
        }
    }

    fn removed(&self, reference: &str) -> Result<(), Status> {
        match self.remove(reference) {
            Status::Ok | Status::Missing => Ok(()),
            status => Err(status),
        }
    }
}

/// The staged copy a move from an earlier build passes through.
fn staged(reference: &str) -> String {
    format!("{reference}-retrust")
}

/// Where `gather` found a key.
#[derive(Debug)]
pub(crate) enum Origin {
    /// The protected item trusts this binary.
    Current,
    /// As `Current`, with an equal staged copy left by an interrupted move.
    CurrentAndStaged,
    /// Only the staged copy of an interrupted move is left.
    Staged,
    /// The protected item trusts an earlier build; a person allowed one read of it.
    Earlier(CFType),
    /// Only the legacy `keyring` item exists.
    Legacy,
}

/// One key's value and where it was found. Its value is not yet verified.
pub(crate) struct Gathered {
    pub reference: String,
    pub origin: Origin,
    pub value: Zeroizing<Vec<u8>>,
}
impl Gathered {
    pub(crate) fn moves(&self) -> bool {
        !matches!(self.origin, Origin::Current)
    }
}

#[cfg(test)]
impl Keychain {
    pub(crate) fn create_staged_for_test(&self, reference: &str, value: &[u8]) -> Status {
        self.create_as(&staged(reference), Class::PinnedSelf, value)
    }
    pub(crate) fn remove_staged_for_test(&self, reference: &str) -> Status {
        self.remove(&staged(reference))
    }
    pub(crate) fn create_legacy_for_test(&self, reference: &str, value: &[u8]) -> Status {
        let own = match own_application() {
            Ok((application, _)) => application,
            Err(status) => return status,
        };
        let trusted = [unsafe { CFType::wrap_under_get_rule(own.0 as CFTypeRef) }];
        self.add(
            LEGACY_SERVICE,
            reference,
            &CFArray::from_CFTypes(&trusted),
            value,
        )
    }
    /// A key item as an earlier build left it: trusting exactly one other binary.
    pub(crate) fn create_earlier_for_test(&self, reference: &str, value: &[u8]) -> Status {
        let path = CString::new(crate::native::tests::fake_helper().to_str().unwrap()).unwrap();
        let mut application = std::ptr::null_mut();
        if unsafe { SecTrustedApplicationCreateFromPath(path.as_ptr(), &mut application) } != 0 {
            return Status::Failed;
        }
        let other = unsafe { CFType::wrap_under_create_rule(application as CFTypeRef) };
        self.add(SERVICE, reference, &CFArray::from_CFTypes(&[other]), value)
    }
    pub(crate) fn remove_legacy_for_test(&self, reference: &str) -> Status {
        self.remove_in(LEGACY_SERVICE, reference)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Migration {
    Legacy(Status),
    Protected(Status),
    Mismatch,
    /// Keys move only with their whole account.
    Refused,
}

impl CredentialBackend for Keychain {
    fn get(&self, reference: &str) -> Result<SecretValue, AcquisitionError> {
        let value = self.read(reference)?;
        Ok(SecretValue::new(value.to_vec()))
    }
    fn put(&self, reference: &str, value: &SecretValue) -> Result<(), AcquisitionError> {
        match self.create(reference, value.bytes()) {
            Status::Ok => Ok(()),
            _ => Err(AcquisitionError::Credentials),
        }
    }
    fn delete(&self, reference: &str) -> Result<(), AcquisitionError> {
        match self.remove(reference) {
            Status::Ok | Status::Missing => Ok(()),
            _ => Err(AcquisitionError::Credentials),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
