use super::lifecycle::{
    boundary, cipher_path, nonce, read_bounded, remove_file, sync_dir, write_json,
};
use super::*;
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use data_encoding::BASE64URL_NOPAD as B64;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// Bound on a version-1 or version-2 bundle, which is one JWE in memory.
/// New bundles are version 3 (`stream`) and hold account files of any size.
const MAX_BUNDLE: u64 = 24 * 1024 * 1024;
const MAX_PLAIN: usize = 16 * 1024 * 1024;
/// Bound on a version-3 bundle: up to eight account files of the store's
/// largest size, plus the encrypted header and chunk framing.
const MAX_STREAM: u64 = 8 * (crate::census::MAX_FILE + crate::census::MAX_FILE / 64) + (8 << 20);
const HEADER: &[u8] = br#"{"alg":"dir","enc":"A256GCM","typ":"mailroom-recovery+jwe","v":1}"#;
pub(crate) const LIMITATIONS: &str = "Only the captured local account is restored. Provider passwords, external mailbox/transport effects and shared authoritative addresses.yaml are excluded. Cross-account dependencies may be unavailable. Later mail effects survive. Fresh provider evidence and reconciliation are required. No blind retry, automatic undo or daily readiness.";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Restored {
    pub authority: String,
    pub bundle: String,
    pub captured_at: u64,
    pub authenticated: bool,
    pub reconciliation_required: bool,
    pub resources: Vec<Inventory>,
    pub key_material: Vec<KeyMaterial>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Inventory {
    pub resource: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KeyMaterial {
    pub resource: String,
    pub bytes: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub version: u32,
    pub state_schema: u32,
    pub captured_at: u64,
    pub account: AccountId,
    pub provider: Provider,
    pub login: String,
    pub generation: u64,
    pub revision: u64,
    pub resources: Vec<Inventory>,
    pub key_material: Vec<KeyMaterial>,
    pub dependencies: BTreeSet<String>,
    pub shared_state: String,
    pub excluded: Vec<String>,
    pub limitations: String,
}
/// A captured account. Its encrypted account file is `resource`: the live
/// file while capturing, or a decrypted copy staged in a private directory
/// after a bundle was opened.
struct Captured {
    manifest: Manifest,
    account: Account,
    key: SecretBytes,
    identity: Option<SecretBytes>,
    resource: Resource,
}
/// Everything of a capture except its account file, as a bundle header.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapturedHeader {
    manifest: Manifest,
    account: Account,
    key: SecretBytes,
    identity: Option<SecretBytes>,
}
/// A capture inside a version-1 or version-2 JWE bundle, file inline.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyCaptured {
    manifest: Manifest,
    account: Account,
    key: SecretBytes,
    identity: Option<SecretBytes>,
    ciphertext: Vec<u8>,
}
/// The header of a version-3 bundle. Its account files follow in order.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Body {
    Single {
        captured: CapturedHeader,
    },
    Selected {
        manifest: selected::SelectedManifest,
        accounts: Vec<CapturedHeader>,
        ledger: Option<SecretBytes>,
    },
}
pub(super) enum Resource {
    Live(PathBuf),
    Staged {
        _directory: std::sync::Arc<tempfile::TempDir>,
        path: PathBuf,
        digest: (u64, String),
    },
}
impl Resource {
    fn path(&self) -> &Path {
        match self {
            Self::Live(path) | Self::Staged { path, .. } => path,
        }
    }
    /// Copies the staged file to `target`, never replacing a file there.
    fn publish(&self, target: &Path) -> Result<(), AcquisitionError> {
        let Self::Staged { path, digest, .. } = self else {
            return Err(AcquisitionError::Storage);
        };
        let dir = target.parent().ok_or(AcquisitionError::Storage)?;
        private_directory(dir)?;
        let mut file = tempfile::Builder::new()
            .prefix(".recovery-stage-")
            .tempfile_in(dir)
            .map_err(|_| AcquisitionError::Storage)?;
        let mut source = fs::File::open(path).map_err(|_| AcquisitionError::Storage)?;
        std::io::copy(&mut source, file.as_file_mut())
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| AcquisitionError::Storage)?;
        if stream::file_digest(file.path())? != *digest {
            return Err(AcquisitionError::Storage);
        }
        boundary("recovery_staged")?;
        file.persist_noclobber(target)
            .map_err(|_| AcquisitionError::Storage)?;
        sync_dir(dir)
    }
    /// Whether the file at `path` holds exactly the captured bytes.
    fn matches(&self, path: &Path, manifest: &Manifest) -> Result<bool, AcquisitionError> {
        let (bytes, sha256) = stream::file_digest(path)?;
        Ok(manifest.resources.iter().any(|r| {
            r.resource == "local_encrypted_state" && r.bytes == bytes && r.sha256 == sha256
        }))
    }
}
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct SecretBytes(#[serde(with = "secret_bytes")] Zeroizing<Vec<u8>>);
impl Clone for SecretBytes {
    fn clone(&self) -> Self {
        Self(Zeroizing::new(self.0.to_vec()))
    }
}
impl Captured {
    fn header(&self) -> CapturedHeader {
        CapturedHeader {
            manifest: self.manifest.clone(),
            account: self.account.clone(),
            key: self.key.clone(),
            identity: self.identity.clone(),
        }
    }
}
mod secret_bytes {
    use super::*;
    pub fn serialize<S: serde::Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(v)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Zeroizing<Vec<u8>>, D::Error> {
        Ok(Zeroizing::new(Vec::<u8>::deserialize(d)?))
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestorePreview {
    pub manifest: Manifest,
    pub bundle: String,
    pub target: String,
    pub account_absent: bool,
    pub shared_state: String,
    pub consequences: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    version: u32,
    bundle: String,
    authority: String,
    preview: RestorePreview,
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(&Sha256::digest(bytes))
}
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_secs()
}
fn encoded(value: &impl Serialize) -> Result<Zeroizing<Vec<u8>>, AcquisitionError> {
    serde_json::to_vec(value)
        .map(Zeroizing::new)
        .map_err(|_| AcquisitionError::Storage)
}
fn publish_new(path: &Path, bytes: &[u8]) -> Result<(), AcquisitionError> {
    let dir = path.parent().ok_or(AcquisitionError::Storage)?;
    private_directory(dir)?;
    let mut file = tempfile::Builder::new()
        .prefix(".recovery-stage-")
        .tempfile_in(dir)
        .map_err(|_| AcquisitionError::Storage)?;
    file.write_all(bytes)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| AcquisitionError::Storage)?;
    boundary("recovery_staged")?;
    file.persist_noclobber(path)
        .map_err(|_| AcquisitionError::Storage)?;
    sync_dir(dir)
}
/// Writes a version-3 bundle into `directory`, verifies it by opening it
/// again, and publishes it under its identity without replacing a file.
fn write_bundle(
    directory: &Path,
    prefix: &str,
    secret: &SecretValue,
    body: &Body,
    captured: &[&Captured],
    staged_boundary: &str,
    verify: impl FnOnce(&Path) -> Result<String, AcquisitionError>,
) -> Result<String, AcquisitionError> {
    private_directory(directory)?;
    let mut writer = stream::Writer::create(directory, prefix, secret, body)?;
    for c in captured {
        let mut file = fs::File::open(c.resource.path()).map_err(|_| AcquisitionError::Storage)?;
        let (bytes, sha256) = writer.file(&mut file)?;
        if inventory(&c.account, &(bytes, sha256))? != c.manifest.resources {
            return Err(AcquisitionError::InvalidInput);
        }
    }
    let (staged, id) = writer.finish()?;
    if verify(staged.path())? != id {
        return Err(AcquisitionError::Storage);
    }
    stream::publish(staged, directory, &id, staged_boundary)?;
    Ok(id)
}
/// Writes a version-1 or version-2 JWE bundle as earlier builds did, so
/// tests prove those bundles stay readable.
#[cfg(test)]
fn seal_legacy(captured: &impl Serialize, secret: &SecretValue) -> Vec<u8> {
    let plain = encoded(captured).unwrap();
    let header = B64.encode(HEADER);
    let mut iv = [0u8; 12];
    OsRng.fill_bytes(&mut iv);
    let encrypted = Aes256Gcm::new_from_slice(secret.bytes())
        .unwrap()
        .encrypt(
            Nonce::from_slice(&iv),
            Payload {
                msg: &plain,
                aad: header.as_bytes(),
            },
        )
        .unwrap();
    let (body, tag) = encrypted.split_at(encrypted.len() - 16);
    format!(
        "{header}..{}.{}.{}",
        B64.encode(&iv),
        B64.encode(body),
        B64.encode(tag)
    )
    .into_bytes()
}
#[cfg(test)]
impl Captured {
    fn legacy(&self) -> LegacyCaptured {
        LegacyCaptured {
            manifest: self.manifest.clone(),
            account: self.account.clone(),
            key: self.key.clone(),
            identity: self.identity.clone(),
            ciphertext: fs::read(self.resource.path()).unwrap(),
        }
    }
}
/// Writes a version-3 bundle without the capture checks, for tampering tests.
#[cfg(test)]
fn reseal(directory: &Path, secret: &SecretValue, body: &Body, files: &[&Path]) -> PathBuf {
    private_directory(directory).unwrap();
    let mut writer = stream::Writer::create(directory, ".test-", secret, body).unwrap();
    for path in files {
        writer.file(&mut fs::File::open(path).unwrap()).unwrap();
    }
    let (staged, id) = writer.finish().unwrap();
    let path = directory.join(format!("{id}.{}", stream::EXTENSION));
    staged.persist(&path).unwrap();
    path
}
/// The decrypted JWE payload of a version-1 or version-2 bundle.
fn open_plain(bytes: &[u8], secret: &SecretValue) -> Result<Zeroizing<Vec<u8>>, AcquisitionError> {
    if bytes.len() as u64 > MAX_BUNDLE || secret.bytes().len() != 32 {
        return Err(AcquisitionError::InvalidInput);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| AcquisitionError::InvalidInput)?;
    let parts: Vec<_> = text.split('.').collect();
    if parts.len() != 5
        || parts[0] != B64.encode(HEADER)
        || !parts[1].is_empty()
        || parts[2].len() != 16
        || parts[4].len() != 22
        || parts[3].len() > MAX_PLAIN * 4 / 3 + 4
    {
        return Err(AcquisitionError::InvalidInput);
    }
    let decode = |s: &str| {
        B64.decode(s.as_bytes())
            .map_err(|_| AcquisitionError::InvalidInput)
    };
    let iv = decode(parts[2])?;
    let tag = decode(parts[4])?;
    let mut body = decode(parts[3])?;
    if iv.len() != 12 || tag.len() != 16 {
        return Err(AcquisitionError::InvalidInput);
    }
    body.extend_from_slice(&tag);
    let cipher =
        Aes256Gcm::new_from_slice(secret.bytes()).map_err(|_| AcquisitionError::InvalidInput)?;
    let plain = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&iv),
                Payload {
                    msg: &body,
                    aad: parts[0].as_bytes(),
                },
            )
            .map_err(|_| AcquisitionError::Authentication)?,
    );
    if plain.len() > MAX_PLAIN {
        return Err(AcquisitionError::InvalidInput);
    }
    Ok(plain)
}
/// A private temporary directory that holds a bundle's decrypted account
/// file while it is authenticated, and until it is published or dropped.
/// The file is still encrypted under the account's own key.
fn staging(account: &AccountId) -> Result<tempfile::TempDir, AcquisitionError> {
    tempfile::Builder::new()
        .prefix(&format!("mailroom-{}.recovery-", account.as_str()))
        .tempdir()
        .map_err(|_| AcquisitionError::Storage)
}
fn stage_file(
    header: CapturedHeader,
    write: impl FnOnce(&mut fs::File) -> Result<(u64, String), AcquisitionError>,
) -> Result<Captured, AcquisitionError> {
    let directory = staging(&header.account.id)?;
    let path = cipher_path(
        directory.path(),
        &header.account.id,
        header.account.generation,
    );
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).map_err(|_| AcquisitionError::Storage)?;
    let digest = write(&mut file)?;
    file.sync_all().map_err(|_| AcquisitionError::Storage)?;
    Ok(Captured {
        manifest: header.manifest,
        account: header.account,
        key: header.key,
        identity: header.identity,
        resource: Resource::Staged {
            _directory: std::sync::Arc::new(directory),
            path,
            digest,
        },
    })
}
fn stage_legacy(legacy: LegacyCaptured) -> Result<Captured, AcquisitionError> {
    let LegacyCaptured {
        manifest,
        account,
        key,
        identity,
        ciphertext,
    } = legacy;
    if ciphertext.len() > 8 * 1024 * 1024 {
        return Err(AcquisitionError::InvalidInput);
    }
    let header = CapturedHeader {
        manifest,
        account,
        key,
        identity,
    };
    stage_file(header, |file| {
        file.write_all(&ciphertext)
            .map_err(|_| AcquisitionError::Storage)?;
        Ok((ciphertext.len() as u64, digest(&ciphertext)))
    })
}
fn stage_streamed(
    header: CapturedHeader,
    reader: &mut stream::Reader,
) -> Result<Captured, AcquisitionError> {
    stage_file(header, |file| {
        let mut out = std::io::BufWriter::new(file);
        let digest = reader.file(&mut out, crate::census::MAX_FILE)?;
        out.flush().map_err(|_| AcquisitionError::Storage)?;
        Ok(digest)
    })
}
/// Opens a single-account bundle of any version and returns the
/// authenticated capture, its account file staged, and the bundle identity.
fn open(bundle: &Path, secret: &SecretValue) -> Result<(Captured, String), AcquisitionError> {
    let (captured, id) = if stream::Reader::is_stream(bundle)? {
        let (mut reader, body) = stream::Reader::open::<Body>(bundle, secret, MAX_STREAM)?;
        let Body::Single { captured } = body else {
            return Err(AcquisitionError::InvalidInput);
        };
        let captured = stage_streamed(captured, &mut reader)?;
        (captured, reader.finish()?)
    } else {
        let bytes = read_bounded(bundle, MAX_BUNDLE)?;
        let legacy: LegacyCaptured = serde_json::from_slice(&open_plain(&bytes, secret)?)
            .map_err(|_| AcquisitionError::InvalidInput)?;
        (stage_legacy(legacy)?, digest(&bytes))
    };
    validate_captured(&captured)?;
    Ok((captured, id))
}
fn validate_captured(captured: &Captured) -> Result<(), AcquisitionError> {
    let m = &captured.manifest;
    let digest = &match &captured.resource {
        Resource::Staged { digest, .. } => digest.clone(),
        Resource::Live(path) => stream::file_digest(path)?,
    };
    if m.version != 1
        || !(1..=crate::census::SCHEMA).contains(&m.state_schema)
        || m.account != captured.account.id
        || m.provider != captured.account.provider
        || m.login != captured.account.login
        || m.generation != captured.account.generation
        || m.resources != inventory(&captured.account, digest)?
        || m.key_material != key_inventory(&captured.account)
        || encoded(&captured.account)?.len() > 4096
        || captured.account.login.is_empty()
        || captured.account.login.len() > 320
        || captured.account.login.chars().any(char::is_control)
        || digest.0 > crate::census::MAX_FILE
        || captured.key.0.len() != 32
        || captured.identity.as_ref().is_some_and(|i| i.0.len() != 128)
        || captured.account.identity_ref.is_some() != captured.identity.is_some()
        // A rotated account captured before identity commitments cannot be authenticated,
        // so it is refused here, before any key is imported, rather than adopted silently.
        || captured.account.uncommitted()
        || m.shared_state != "excluded_preserve_current"
        || m.excluded != exclusions()
        || m.limitations != LIMITATIONS
        || m.dependencies.len() > 64
    {
        return Err(AcquisitionError::InvalidInput);
    }
    match captured.resource {
        Resource::Staged { .. } => authenticate_resource(captured),
        // Loaded and authenticated under the account's lock while capturing.
        Resource::Live(_) => Ok(()),
    }
}
fn exclusions() -> Vec<String> {
    [
        "provider_passwords",
        "external_mailboxes_and_transports",
        "authoritative_shared_ledger",
        "uncaptured_changes",
    ]
    .map(String::from)
    .to_vec()
}
fn key_inventory(account: &Account) -> Vec<KeyMaterial> {
    let mut keys = vec![KeyMaterial {
        resource: "local_decryption_key".into(),
        bytes: 32,
    }];
    if account.identity_ref.is_some() {
        keys.push(KeyMaterial {
            resource: "stable_identity_material".into(),
            bytes: 128,
        });
    }
    keys
}
fn authenticate_resource(captured: &Captured) -> Result<(), AcquisitionError> {
    let secret = LocalSecret::from_bytes(captured.key.0.to_vec())?;
    let keys = match &captured.identity {
        Some(identity) => CensusKeys::rotated(
            &secret,
            &captured.account.id,
            &identity.0,
            captured.account.generation,
        )?,
        None if captured.account.generation == 0 => {
            CensusKeys::derive(&secret, &captured.account.id)
        }
        _ => return Err(AcquisitionError::InvalidInput),
    };
    if captured.identity.is_some()
        && !keys.commits_to(captured.account.identity_commitment.as_deref())
    {
        return Err(AcquisitionError::InvalidInput);
    }
    let staged = captured.resource.path();
    let store = EncryptedStore::new(
        staged
            .parent()
            .ok_or(AcquisitionError::Storage)?
            .to_path_buf(),
        keys,
    )?;
    if store.path() != staged {
        return Err(AcquisitionError::InvalidInput);
    }
    let saved = store.load_state()?;
    if store.schema_version()? != captured.manifest.state_schema
        || saved.revision != captured.manifest.revision
        || saved.census.account != captured.manifest.account
        || saved
            .winddown
            .settings
            .comparisons
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
            != captured.manifest.dependencies
    {
        return Err(AcquisitionError::Authentication);
    }
    Ok(())
}
fn inventory(account: &Account, file: &(u64, String)) -> Result<Vec<Inventory>, AcquisitionError> {
    let metadata = encoded(account)?;
    Ok(vec![
        Inventory {
            resource: "account_metadata".into(),
            bytes: metadata.len() as u64,
            sha256: digest(&metadata),
        },
        Inventory {
            resource: "local_encrypted_state".into(),
            bytes: file.0,
            sha256: file.1.clone(),
        },
    ])
}

impl Accounts {
    fn capture_locked(
        &self,
        id: &AccountId,
        captured_at: u64,
    ) -> Result<(Captured, EncryptedStore), AcquisitionError> {
        self.require_ready(id)?;
        let account = self.load_metadata(id)?;
        let store = self.open_store(&account, &self.paths.data, 0)?;
        let saved = store.load_state()?;
        let file = stream::file_digest(&store.path())?;
        if file.0 > crate::census::MAX_FILE {
            return Err(AcquisitionError::Storage);
        }
        let key = SecretBytes(Zeroizing::new(
            self.backend
                .get(&account.local_secret_ref)?
                .bytes()
                .to_vec(),
        ));
        let identity = account
            .identity_ref
            .as_ref()
            .map(|r| {
                self.backend
                    .get(r)
                    .map(|s| SecretBytes(Zeroizing::new(s.bytes().to_vec())))
            })
            .transpose()?;
        let dependencies = saved
            .winddown
            .settings
            .comparisons
            .keys()
            .cloned()
            .collect();
        let manifest = Manifest {
            version: 1,
            state_schema: store.schema_version()?,
            captured_at,
            account: id.clone(),
            provider: account.provider,
            login: account.login.clone(),
            generation: account.generation,
            revision: saved.revision,
            resources: inventory(&account, &file)?,
            key_material: key_inventory(&account),
            dependencies,
            shared_state: "excluded_preserve_current".into(),
            excluded: exclusions(),
            limitations: LIMITATIONS.into(),
        };
        Ok((
            Captured {
                manifest,
                account,
                key,
                identity,
                resource: Resource::Live(store.path()),
            },
            store,
        ))
    }
    pub(crate) fn capture_preview(
        &self,
        id: &AccountId,
        captured_at: u64,
    ) -> Result<(Manifest, crate::confirmation::Plan), AcquisitionError> {
        let _lease = self.lock_account(id)?;
        let (captured, store) = self.capture_locked(id, captured_at)?;
        let receipt = store.keys().binding_for(
            b"mailroom/recovery-capture/v1",
            &encoded(&captured.manifest)?,
        );
        let plan = recovery_plan(id, "capture", &captured.manifest, receipt)?;
        Ok((captured.manifest, plan))
    }
    pub(crate) fn capture(
        &self,
        preview: &Manifest,
        secret: &SecretValue,
        directory: &Path,
        authorize: impl FnOnce(crate::confirmation::Plan) -> Result<(), AcquisitionError>,
    ) -> Result<String, AcquisitionError> {
        let _lease = self.lock_account(&preview.account)?;
        let (captured, store) = self.capture_locked(&preview.account, preview.captured_at)?;
        if captured.manifest != *preview {
            return Err(AcquisitionError::InvalidInput);
        }
        let receipt = store
            .keys()
            .binding_for(b"mailroom/recovery-capture/v1", &encoded(preview)?);
        authorize(recovery_plan(
            &preview.account,
            "capture",
            preview,
            receipt,
        )?)?;
        let id = write_bundle(
            directory,
            ".recovery-stage-",
            secret,
            &Body::Single {
                captured: captured.header(),
            },
            &[&captured],
            "recovery_staged",
            |staged| open(staged, secret).map(|(_, id)| id),
        )?;
        boundary("recovery_bundle_published")?;
        Ok(id)
    }
    pub(crate) fn restore_preview(
        &self,
        bundle: &Path,
        secret: &SecretValue,
    ) -> Result<RestorePreview, AcquisitionError> {
        let (captured, id) = open(bundle, secret)?;
        self.validate_account(&captured.account, &captured.manifest.account)?;
        let _lease = self.lock_account(&captured.manifest.account)?;
        self.absent(&captured.account)?;
        Ok(RestorePreview {
            manifest: captured.manifest,
            bundle: id,
            target: self.paths.config.to_string_lossy().into_owned(),
            account_absent: true,
            shared_state: "excluded_preserve_current".into(),
            consequences: LIMITATIONS.into(),
        })
    }
    fn absent(&self, a: &Account) -> Result<(), AcquisitionError> {
        self.require_ready(&a.id)?;
        if self.metadata_path(&a.id).exists()
            || cipher_path(&self.paths.data, &a.id, a.generation).exists()
            || self.list()?.iter().any(|other| {
                other.provider == a.provider
                    && other.login.trim().eq_ignore_ascii_case(a.login.trim())
            })
        {
            return Err(AcquisitionError::InvalidInput);
        }
        for reference in [&a.local_secret_ref, &a.password_ref]
            .into_iter()
            .chain(a.identity_ref.iter())
        {
            match self.backend.get(reference) {
                Err(AcquisitionError::CredentialMissing) => (),
                _ => return Err(AcquisitionError::Credentials),
            }
        }
        Ok(())
    }
    pub(crate) fn restore(
        &self,
        bundle: &Path,
        secret: &SecretValue,
        preview: &RestorePreview,
        authorize: impl FnOnce(crate::confirmation::Plan) -> Result<(), AcquisitionError>,
    ) -> Result<Restored, AcquisitionError> {
        let _installation = self.lock_installation()?;
        let (captured, id) = open(bundle, secret)?;
        let _lease = self.lock_account(&captured.account.id)?;
        self.validate_account(&captured.account, &captured.manifest.account)?;
        self.absent(&captured.account)?;
        let current = RestorePreview {
            manifest: captured.manifest.clone(),
            bundle: id,
            target: self.paths.config.to_string_lossy().into_owned(),
            account_absent: true,
            shared_state: "excluded_preserve_current".into(),
            consequences: LIMITATIONS.into(),
        };
        if current != *preview {
            return Err(AcquisitionError::InvalidInput);
        }
        authorize(restore_plan(&current)?)?;
        let tx = Transaction {
            version: 1,
            bundle: current.bundle.clone(),
            authority: nonce(),
            preview: current,
        };
        self.stage_bundle(bundle, &tx.bundle)?;
        let marker = self.restore_marker(&captured.account.id);
        write_json(&marker, &tx)?;
        boundary("restore_intent")?;
        self.finish_restore(captured, &tx)
    }
    /// Where an authenticated bundle is kept while its restore may resume.
    fn staged_bundle(&self, id: &str) -> Result<PathBuf, AcquisitionError> {
        bundle_file(&self.paths.config.join("recovery"), id)
    }
    /// Copies `bundle` beside the restore marker, or checks an existing copy.
    fn stage_bundle(&self, bundle: &Path, id: &str) -> Result<(), AcquisitionError> {
        let directory = self.paths.config.join("recovery");
        if let Ok(existing) = self.staged_bundle(id) {
            return if stream::file_digest(&existing)?.1 == id {
                Ok(())
            } else {
                Err(AcquisitionError::Storage)
            };
        }
        private_directory(&directory)?;
        let extension = if stream::Reader::is_stream(bundle)? {
            stream::EXTENSION
        } else {
            "jwe"
        };
        let mut staged = tempfile::Builder::new()
            .prefix(".recovery-stage-")
            .tempfile_in(&directory)
            .map_err(|_| AcquisitionError::Storage)?;
        let mut source = fs::File::open(bundle).map_err(|_| AcquisitionError::Storage)?;
        std::io::copy(&mut source, staged.as_file_mut())
            .and_then(|_| staged.as_file().sync_all())
            .map_err(|_| AcquisitionError::Storage)?;
        if stream::file_digest(staged.path())?.1 != id {
            return Err(AcquisitionError::Storage);
        }
        boundary("recovery_staged")?;
        staged
            .persist_noclobber(directory.join(format!("{id}.{extension}")))
            .map_err(|_| AcquisitionError::Storage)?;
        sync_dir(&directory)
    }
    fn restore_marker(&self, id: &AccountId) -> PathBuf {
        self.paths
            .config
            .join("lifecycle")
            .join(format!("{}.restore", id.as_str()))
    }
    pub(crate) fn resume_restore(
        &self,
        id: &AccountId,
        secret: &SecretValue,
    ) -> Result<Restored, AcquisitionError> {
        let _installation = self.lock_installation()?;
        let _lease = self.lock_account(id)?;
        let tx: Transaction =
            serde_json::from_slice(&read_bounded(&self.restore_marker(id), 64 * 1024)?)
                .map_err(|_| AcquisitionError::Storage)?;
        if tx.version != 1
            || !valid_digest(&tx.bundle)
            || tx.preview.manifest.account != *id
            || tx.preview.bundle != tx.bundle
            || tx.preview.target != self.paths.config.to_string_lossy()
            || tx.authority.len() != 32
        {
            return Err(AcquisitionError::InvalidInput);
        }
        let (captured, bundle) = open(&self.staged_bundle(&tx.bundle)?, secret)?;
        if bundle != tx.bundle {
            return Err(AcquisitionError::Authentication);
        }
        if captured.manifest != tx.preview.manifest {
            return Err(AcquisitionError::InvalidInput);
        }
        self.validate_account(&captured.account, id)?;
        self.finish_restore(captured, &tx)
    }
    fn finish_restore(
        &self,
        mut captured: Captured,
        tx: &Transaction,
    ) -> Result<Restored, AcquisitionError> {
        let a = &mut captured.account;
        let restored = Restored {
            authority: tx.authority.clone(),
            bundle: tx.bundle.clone(),
            captured_at: tx.preview.manifest.captured_at,
            authenticated: true,
            reconciliation_required: true,
            resources: captured.manifest.resources.clone(),
            key_material: captured.manifest.key_material.clone(),
        };
        a.connection = ConnectionState::Unverified;
        a.restored = Some(restored.clone());
        self.put_exact(&a.local_secret_ref, &captured.key.0)?;
        boundary("restore_key")?;
        if let (Some(reference), Some(identity)) = (&a.identity_ref, &captured.identity) {
            self.put_exact(reference, &identity.0)?;
        }
        boundary("restore_identity")?;
        boundary("restore_authenticated")?;
        let locked = self.open_store(a, &self.paths.data, 0)?;
        if locked.path().exists() {
            if !captured
                .resource
                .matches(&locked.path(), &captured.manifest)?
            {
                return Err(AcquisitionError::Storage);
            }
        } else {
            captured.resource.publish(&locked.path())?;
        }
        locked.load_state()?;
        boundary("restore_data")?;
        if self.metadata_path(&a.id).exists() {
            if encoded(&self.load_metadata(&a.id)?)?.as_slice() != encoded(a)?.as_slice() {
                return Err(AcquisitionError::Storage);
            }
        } else {
            publish_new(&self.metadata_path(&a.id), &encoded(a)?)?;
        }
        boundary("restore_metadata")?;
        remove_file(&self.restore_marker(&a.id))?;
        boundary("restore_committed")?;
        Ok(restored)
    }
    fn put_exact(&self, reference: &str, bytes: &[u8]) -> Result<(), AcquisitionError> {
        use subtle::ConstantTimeEq;
        match self.backend.get(reference) {
            Ok(value) if bool::from(value.bytes().ct_eq(bytes)) => Ok(()),
            Err(AcquisitionError::CredentialMissing) => self
                .backend
                .put(reference, &SecretValue::new(bytes.to_vec())),
            _ => Err(AcquisitionError::Credentials),
        }
    }
    pub(crate) fn recovery_view(&self) -> Result<serde_json::Value, AcquisitionError> {
        let mut pending = vec![];
        let root = self.paths.config.join("lifecycle");
        if root.exists() {
            for entry in fs::read_dir(root).map_err(|_| AcquisitionError::Storage)? {
                let path = entry.map_err(|_| AcquisitionError::Storage)?.path();
                if path.extension().is_some_and(|x| x == "restore") {
                    pending.push(AccountId::new(
                        path.file_stem()
                            .and_then(|s| s.to_str())
                            .ok_or(AcquisitionError::Storage)?,
                    )?);
                }
            }
        }
        Ok(
            serde_json::json!({"pending":pending,"restored":self.list()?.into_iter().filter_map(|a| a.restored.map(|r| serde_json::json!({"account":a.id,"captured_at":r.captured_at,"authenticated":r.authenticated,"reconciliation_required":r.reconciliation_required,"bundle":r.bundle,"resources":r.resources,"key_material":r.key_material}))).collect::<Vec<_>>(),"production_available":false,"limitations":LIMITATIONS}),
        )
    }
}
/// The bundle `id` in `directory`: a version-3 `.bundle` or an older `.jwe`.
pub(crate) fn bundle_file(directory: &Path, id: &str) -> Result<PathBuf, AcquisitionError> {
    if !valid_digest(id) {
        return Err(AcquisitionError::InvalidInput);
    }
    [stream::EXTENSION, "jwe"]
        .iter()
        .map(|extension| directory.join(format!("{id}.{extension}")))
        .find(|path| path.is_file())
        .ok_or(AcquisitionError::Storage)
}
pub(crate) fn valid_digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn recovery_plan(
    id: &AccountId,
    operation: &str,
    value: &impl Serialize,
    receipt: String,
) -> Result<crate::confirmation::Plan, AcquisitionError> {
    Ok(crate::confirmation::Plan {
        actions: vec![crate::confirmation::ExactAction::Recovery {
            account: id.as_str().into(),
            operation: operation.into(),
            manifest_digest: digest(&encoded(value)?),
            consequences: LIMITATIONS.into(),
        }],
        exact_receipt: receipt,
    })
}
pub(crate) fn restore_plan(
    p: &RestorePreview,
) -> Result<crate::confirmation::Plan, AcquisitionError> {
    recovery_plan(
        &p.manifest.account,
        "restore_absent_account",
        p,
        digest(&encoded(p)?),
    )
}

#[cfg(test)]
mod tests;

pub(crate) mod selected;
mod stream;
