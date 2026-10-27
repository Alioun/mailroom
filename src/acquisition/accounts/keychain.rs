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
    ffi::{CString, c_char, c_void},
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

fn search_list_paths() -> Vec<PathBuf> {
    let mut list = std::ptr::null();
    if unsafe { SecKeychainCopySearchList(&mut list) } != 0 || list.is_null() {
        return vec![];
    }
    let list: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(list) };
    list.iter()
        .filter_map(|keychain| {
            let mut buffer = [0 as c_char; 1024];
            let mut length = buffer.len() as u32;
            let status = unsafe {
                SecKeychainGetPath(
                    keychain.as_CFTypeRef() as *mut c_void,
                    &mut length,
                    buffer.as_mut_ptr(),
                )
            };
            (status == 0).then(|| {
                let path = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) };
                PathBuf::from(path.to_string_lossy().into_owned())
            })
        })
        .filter_map(|p| p.canonicalize().ok())
        .collect()
}

/// An explicit keychain file for tests and checks. It must already exist and must not be
/// a system or user keychain; nothing ever falls back to the default keychain.
pub(crate) fn explicit(path: &Path) -> Result<Keychain, Status> {
    if !path.is_absolute() {
        return Err(Status::Failed);
    }
    let canonical = path.canonicalize().map_err(|_| Status::Failed)?;
    let meta = std::fs::symlink_metadata(&canonical).map_err(|_| Status::Failed)?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let protected = ["/Library/Keychains", "/System", "/private/var/db"]
        .iter()
        .map(PathBuf::from)
        .chain(home.map(|h| h.join("Library/Keychains")))
        .filter_map(|root| root.canonicalize().ok().or(Some(root)))
        .any(|root| canonical.starts_with(root));
    if !meta.is_file() || protected || search_list_paths().contains(&canonical) {
        return Err(Status::Failed);
    }
    let text =
        CString::new(canonical.to_str().ok_or(Status::Failed)?).map_err(|_| Status::Failed)?;
    let mut keychain = std::ptr::null_mut();
    if unsafe { SecKeychainOpen(text.as_ptr(), &mut keychain) } != 0 || keychain.is_null() {
        return Err(Status::Failed);
    }
    let keychain = unsafe { CFType::wrap_under_create_rule(keychain as CFTypeRef) };
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

    /// Reduces every access list that grants more than encryption to one trusted set.
    fn observe(item: &CFType) -> Observed {
        let mut access = std::ptr::null_mut();
        if unsafe {
            SecKeychainItemCopyAccess(item.as_CFTypeRef() as SecKeychainItemRef, &mut access)
        } != 0
            || access.is_null()
        {
            return Observed::Changed;
        }
        let access = Owned(access);
        let mut list = std::ptr::null();
        if unsafe { SecAccessCopyACLList(access.0, &mut list) } != 0 || list.is_null() {
            return Observed::Changed;
        }
        let list: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(list) };
        let mut reading: Option<Vec<String>> = None;
        for acl in list.iter() {
            let acl = acl.as_CFTypeRef() as SecAclRef;
            let authorizations = unsafe { SecACLCopyAuthorizations(acl) };
            if authorizations.is_null() {
                return Observed::Changed;
            }
            let authorizations: CFArray<CFString> =
                unsafe { CFArray::wrap_under_create_rule(authorizations) };
            let authorizations: Vec<String> =
                authorizations.iter().map(|a| a.to_string()).collect();
            if authorizations
                .iter()
                .all(|a| a == "ACLAuthorizationEncrypt")
            {
                continue;
            }
            let mut applications = std::ptr::null();
            let mut description = std::ptr::null();
            let mut selector = 0u16;
            if unsafe {
                SecACLCopyContents(acl, &mut applications, &mut description, &mut selector)
            } != 0
            {
                return Observed::Changed;
            }
            if !description.is_null() {
                drop(unsafe { CFString::wrap_under_create_rule(description) });
            }
            if applications.is_null() {
                return Observed::Changed;
            }
            let applications: CFArray<CFType> =
                unsafe { CFArray::wrap_under_create_rule(applications) };
            let mut trusted = Vec::new();
            for application in applications.iter() {
                match requirement_of(application.as_CFTypeRef() as SecTrustedApplicationRef) {
                    Some(requirement) => trusted.push(requirement),
                    None => return Observed::Changed,
                }
            }
            if authorizations
                .iter()
                .all(|a| a == "ACLAuthorizationChangeACL")
            {
                if !trusted.is_empty() {
                    return Observed::Changed;
                }
                continue;
            }
            match &reading {
                None => reading = Some(trusted),
                Some(previous) if *previous == trusted => {}
                Some(_) => return Observed::Changed,
            }
        }
        match reading {
            Some(trusted) if trusted.is_empty() => Observed::EveryAccess,
            Some(mut trusted) if trusted.len() == 1 => Observed::Pinned(trusted.remove(0)),
            _ => Observed::Changed,
        }
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

    /// Copies a legacy value into a protected item, then proves the protected item holds it.
    /// Returns `Ok` when created and `Exists` when an equal item was already there.
    pub(crate) fn migrate(&self, reference: &str) -> Result<Status, Migration> {
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

    /// Moves a key item that trusts another binary, such as an earlier mailroom build, to
    /// this binary. Reading the old item needs a person. The value is staged in a second
    /// pinned item first, so an interruption at any step leaves a readable copy and a
    /// rerun finishes. Returns `Ok` when moved and `Exists` when nothing needed moving.
    pub(crate) fn retrust(&self, reference: &str) -> Result<Status, Status> {
        if !valid_reference(reference) || Class::of(reference) != Class::PinnedSelf {
            return Err(Status::Failed);
        }
        let staged = format!("{reference}-retrust");
        match self.status(reference) {
            Status::Ok => {
                if self.status_as(&staged, Class::PinnedSelf) != Status::Missing {
                    let current = self.read(reference)?;
                    let copy = self.read_as(&staged, Class::PinnedSelf)?;
                    if !equal(&current, &copy) {
                        return Err(Status::Changed);
                    }
                    self.removed(&staged)?;
                }
                Ok(Status::Exists)
            }
            Status::Missing => {
                let value = self.read_as(&staged, Class::PinnedSelf)?;
                self.publish(reference, &staged, &value)
            }
            Status::OtherBinary => {
                let item = self.item(SERVICE, reference)?;
                let value = self.data(&item)?;
                let kept = match self.status_as(&staged, Class::PinnedSelf) {
                    Status::Missing => false,
                    Status::Ok => equal(&self.read_as(&staged, Class::PinnedSelf)?, &value),
                    _ => false,
                };
                if !kept {
                    self.removed(&staged)?;
                    match self.create_as(&staged, Class::PinnedSelf, &value) {
                        Status::Ok => {}
                        status => return Err(status),
                    }
                }
                if !equal(&self.read_as(&staged, Class::PinnedSelf)?, &value) {
                    return Err(Status::Failed);
                }
                match Status::of(unsafe {
                    SecKeychainItemDelete(item.as_CFTypeRef() as SecKeychainItemRef)
                }) {
                    Status::Ok => {}
                    status => return Err(status),
                }
                self.publish(reference, &staged, &value)
            }
            status => Err(status),
        }
    }

    fn publish(&self, reference: &str, staged: &str, value: &[u8]) -> Result<Status, Status> {
        match self.create(reference, value) {
            Status::Ok => {}
            status => return Err(status),
        }
        if !equal(&self.read(reference)?, value) {
            return Err(Status::Failed);
        }
        self.removed(staged)?;
        Ok(Status::Ok)
    }

    fn removed(&self, reference: &str) -> Result<(), Status> {
        match self.remove(reference) {
            Status::Ok | Status::Missing => Ok(()),
            status => Err(status),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Migration {
    Legacy(Status),
    Protected(Status),
    Mismatch,
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
mod tests;
