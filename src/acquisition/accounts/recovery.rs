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

const MAX_BUNDLE: u64 = 24 * 1024 * 1024;
const MAX_PLAIN: usize = 16 * 1024 * 1024;
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
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Captured {
    manifest: Manifest,
    account: Account,
    key: SecretBytes,
    identity: Option<SecretBytes>,
    ciphertext: Vec<u8>,
}
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct SecretBytes(#[serde(with = "secret_bytes")] Zeroizing<Vec<u8>>);
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
fn seal(captured: &impl Serialize, secret: &SecretValue) -> Result<Vec<u8>, AcquisitionError> {
    if secret.bytes().len() != 32 {
        return Err(AcquisitionError::InvalidInput);
    }
    let plain = encoded(captured)?;
    if plain.len() > MAX_PLAIN {
        return Err(AcquisitionError::InvalidInput);
    }
    let header = B64.encode(HEADER);
    let mut iv = [0u8; 12];
    OsRng.fill_bytes(&mut iv);
    let cipher =
        Aes256Gcm::new_from_slice(secret.bytes()).map_err(|_| AcquisitionError::InvalidInput)?;
    let encrypted = cipher
        .encrypt(
            Nonce::from_slice(&iv),
            Payload {
                msg: &plain,
                aad: header.as_bytes(),
            },
        )
        .map_err(|_| AcquisitionError::Storage)?;
    let (body, tag) = encrypted.split_at(encrypted.len() - 16);
    Ok(format!(
        "{header}..{}.{}.{}",
        B64.encode(&iv),
        B64.encode(body),
        B64.encode(tag)
    )
    .into_bytes())
}
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
fn open(bytes: &[u8], secret: &SecretValue) -> Result<Captured, AcquisitionError> {
    let captured: Captured = serde_json::from_slice(&open_plain(bytes, secret)?)
        .map_err(|_| AcquisitionError::InvalidInput)?;
    validate_captured(&captured)?;
    Ok(captured)
}
fn validate_captured(captured: &Captured) -> Result<(), AcquisitionError> {
    let m = &captured.manifest;
    if m.version != 1
        || !(1..=16).contains(&m.state_schema)
        || m.account != captured.account.id
        || m.provider != captured.account.provider
        || m.login != captured.account.login
        || m.generation != captured.account.generation
        || m.resources != inventory(&captured.account, &captured.ciphertext)?
        || m.key_material != key_inventory(&captured.account)
        || encoded(&captured.account)?.len() > 4096
        || captured.account.login.is_empty()
        || captured.account.login.len() > 320
        || captured.account.login.chars().any(char::is_control)
        || captured.ciphertext.len() > 8 * 1024 * 1024
        || captured.key.0.len() != 32
        || captured.identity.as_ref().is_some_and(|i| i.0.len() != 128)
        || captured.account.identity_ref.is_some() != captured.identity.is_some()
        || m.shared_state != "excluded_preserve_current"
        || m.excluded != exclusions()
        || m.limitations != LIMITATIONS
        || m.dependencies.len() > 64
    {
        return Err(AcquisitionError::InvalidInput);
    }
    authenticate_resource(captured)
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
    let isolated = tempfile::Builder::new()
        .prefix(&format!(
            "mailroom-{}.recovery-auth-",
            captured.account.id.as_str()
        ))
        .tempdir()
        .map_err(|_| AcquisitionError::Storage)?;
    let store = EncryptedStore::new(isolated.path().to_path_buf(), keys)?;
    publish_new(&store.path(), &captured.ciphertext)?;
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
fn inventory(account: &Account, ciphertext: &[u8]) -> Result<Vec<Inventory>, AcquisitionError> {
    let metadata = encoded(account)?;
    Ok(vec![
        Inventory {
            resource: "account_metadata".into(),
            bytes: metadata.len() as u64,
            sha256: digest(&metadata),
        },
        Inventory {
            resource: "local_encrypted_state".into(),
            bytes: ciphertext.len() as u64,
            sha256: digest(ciphertext),
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
        let ciphertext = read_bounded(&store.path(), 8 * 1024 * 1024)?;
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
            resources: inventory(&account, &ciphertext)?,
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
                ciphertext,
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
        let bytes = seal(&captured, secret)?;
        open(&bytes, secret)?;
        let id = digest(&bytes);
        publish_new(&directory.join(format!("{id}.jwe")), &bytes)?;
        boundary("recovery_bundle_published")?;
        Ok(id)
    }
    pub(crate) fn restore_preview(
        &self,
        bundle: &Path,
        secret: &SecretValue,
    ) -> Result<RestorePreview, AcquisitionError> {
        let bytes = read_bounded(bundle, MAX_BUNDLE)?;
        let captured = open(&bytes, secret)?;
        self.validate_account(&captured.account, &captured.manifest.account)?;
        let _lease = self.lock_account(&captured.manifest.account)?;
        self.absent(&captured.account)?;
        Ok(RestorePreview {
            manifest: captured.manifest,
            bundle: digest(&bytes),
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
        let bytes = read_bounded(bundle, MAX_BUNDLE)?;
        let captured = open(&bytes, secret)?;
        let _lease = self.lock_account(&captured.account.id)?;
        self.validate_account(&captured.account, &captured.manifest.account)?;
        self.absent(&captured.account)?;
        let current = RestorePreview {
            manifest: captured.manifest.clone(),
            bundle: digest(&bytes),
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
        let staged = self
            .paths
            .config
            .join("recovery")
            .join(format!("{}.jwe", tx.bundle));
        if staged.exists() {
            if read_bounded(&staged, MAX_BUNDLE)? != bytes {
                return Err(AcquisitionError::Storage);
            }
        } else {
            publish_new(&staged, &bytes)?;
        }
        let marker = self.restore_marker(&captured.account.id);
        write_json(&marker, &tx)?;
        boundary("restore_intent")?;
        self.finish_restore(captured, &tx)
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
        let bytes = read_bounded(
            &self
                .paths
                .config
                .join("recovery")
                .join(format!("{}.jwe", tx.bundle)),
            MAX_BUNDLE,
        )?;
        if digest(&bytes) != tx.bundle {
            return Err(AcquisitionError::Authentication);
        }
        let captured = open(&bytes, secret)?;
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
            if read_bounded(&locked.path(), 8 * 1024 * 1024)? != captured.ciphertext {
                return Err(AcquisitionError::Storage);
            }
        } else {
            publish_new(&locked.path(), &captured.ciphertext)?;
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
