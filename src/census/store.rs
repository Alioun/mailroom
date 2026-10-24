use super::{AccountId, Census, CensusError};
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::{RngCore, rngs::OsRng};
use rusqlite::Connection;
use sha2::Sha256;
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
};
use zeroize::Zeroizing;

const MAGIC: &[u8] = b"MAILROOM-CENSUS\x01";
const MAX_SNAPSHOT: u64 = 128 * 1024 * 1024;

pub struct LocalSecret(Zeroizing<Vec<u8>>);

impl LocalSecret {
    pub fn random() -> Self {
        let mut secret = Zeroizing::new(vec![0; 32]);
        OsRng.fill_bytes(&mut secret);
        Self(secret)
    }
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, CensusError> {
        let bytes = Zeroizing::new(bytes);
        if bytes.len() != 32 {
            return Err(CensusError::Authentication);
        }
        Ok(Self(bytes))
    }
}

impl std::fmt::Debug for LocalSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LocalSecret([REDACTED])")
    }
}

pub struct CensusKeys {
    account: AccountId,
    encryption: Zeroizing<[u8; 32]>,
    placeholders: Zeroizing<[u8; 32]>,
}

impl CensusKeys {
    pub(super) fn belongs_to(&self, account: &AccountId) -> bool {
        self.account == *account
    }

    pub fn derive(secret: &LocalSecret, account: &AccountId) -> Self {
        let hkdf = Hkdf::<Sha256>::new(Some(account.as_str().as_bytes()), &secret.0);
        let mut encryption = Zeroizing::new([0; 32]);
        let mut placeholders = Zeroizing::new([0; 32]);
        hkdf.expand(b"mailroom/census/v1/aes-256-gcm", encryption.as_mut())
            .expect("valid HKDF length");
        hkdf.expand(b"mailroom/census/v1/hmac-sha256", placeholders.as_mut())
            .expect("valid HKDF length");
        Self {
            account: account.clone(),
            encryption,
            placeholders,
        }
    }

    pub fn placeholder(&self, kind: &str, value: &str) -> String {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(self.placeholders.as_ref())
            .expect("valid HMAC key");
        mac.update(kind.as_bytes());
        mac.update(&[0]);
        mac.update(value.trim().to_lowercase().as_bytes());
        let encoded = data_encoding::BASE32_NOPAD.encode(&mac.finalize().into_bytes());
        format!("{kind}:{}", encoded[..6].to_ascii_lowercase())
    }
}

pub struct EncryptedStore {
    directory: PathBuf,
    keys: CensusKeys,
}

impl EncryptedStore {
    pub fn new(directory: PathBuf, keys: CensusKeys) -> Result<Self, CensusError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&directory)
                .map_err(|_| CensusError::Storage)?;
        }
        #[cfg(not(unix))]
        fs::create_dir_all(&directory).map_err(|_| CensusError::Storage)?;
        let metadata = fs::symlink_metadata(&directory).map_err(|_| CensusError::Storage)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(CensusError::Storage);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .map_err(|_| CensusError::Storage)?;
        }
        Ok(Self { directory, keys })
    }

    pub fn path(&self) -> PathBuf {
        self.directory
            .join(format!("{}.census.enc", self.keys.account.as_str()))
    }

    fn context(&self) -> Vec<u8> {
        [MAGIC, self.keys.account.as_str().as_bytes()].concat()
    }

    pub fn save(&self, census: &Census) -> Result<(), CensusError> {
        self.stage(census)?
            .persist(self.path())
            .map_err(|_| CensusError::Storage)?;
        fs::File::open(&self.directory)
            .and_then(|f| f.sync_all())
            .map_err(|_| CensusError::Storage)?;
        Ok(())
    }

    fn stage(&self, census: &Census) -> Result<tempfile::NamedTempFile, CensusError> {
        if census.account != self.keys.account {
            return Err(CensusError::AccountMismatch);
        }
        // Authenticate any existing snapshot before replacement, including after a failed open.
        if self.path().try_exists().map_err(|_| CensusError::Storage)? {
            self.load()?;
        }
        let db = memory_database()?;
        db.execute_batch("CREATE TABLE census (id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL, account TEXT NOT NULL, snapshot TEXT NOT NULL)")
            .map_err(|_| CensusError::Storage)?;
        let snapshot =
            Zeroizing::new(serde_json::to_string(census).map_err(|_| CensusError::Storage)?);
        db.execute(
            "INSERT INTO census VALUES(1, 1, ?1, ?2)",
            (self.keys.account.as_str(), snapshot.as_str()),
        )
        .map_err(|_| CensusError::Storage)?;
        let sqlite = db.serialize("main").map_err(|_| CensusError::Storage)?;
        if sqlite.len() as u64 > MAX_SNAPSHOT {
            return Err(CensusError::Storage);
        }
        let cipher = Aes256Gcm::new_from_slice(self.keys.encryption.as_ref()).expect("AES-256 key");
        let mut nonce = [0; 12];
        OsRng.fill_bytes(&mut nonce);
        let encrypted = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &sqlite,
                    aad: &self.context(),
                },
            )
            .map_err(|_| CensusError::Storage)?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(&self.directory).map_err(|_| CensusError::Storage)?;
        temporary
            .write_all(MAGIC)
            .and_then(|_| temporary.write_all(&nonce))
            .and_then(|_| temporary.write_all(&encrypted))
            .map_err(|_| CensusError::Storage)?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|_| CensusError::Storage)?;
        Ok(temporary)
    }

    pub fn load(&self) -> Result<Census, CensusError> {
        let path = self.path();
        let metadata = fs::symlink_metadata(&path).map_err(|_| CensusError::Storage)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_SNAPSHOT + 128
        {
            return Err(CensusError::Storage);
        }
        let mut bytes = Vec::new();
        fs::File::open(path)
            .and_then(|f| f.take(MAX_SNAPSHOT + 129).read_to_end(&mut bytes))
            .map_err(|_| CensusError::Storage)?;
        if bytes.len() < MAGIC.len() + 12 + 16 {
            return Err(CensusError::Authentication);
        }
        if !bytes.starts_with(MAGIC) {
            return Err(CensusError::UnsupportedVersion);
        }
        let cipher = Aes256Gcm::new_from_slice(self.keys.encryption.as_ref()).expect("AES-256 key");
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    Nonce::from_slice(&bytes[MAGIC.len()..MAGIC.len() + 12]),
                    Payload {
                        msg: &bytes[MAGIC.len() + 12..],
                        aad: &self.context(),
                    },
                )
                .map_err(|_| CensusError::Authentication)?,
        );
        let mut db = memory_database()?;
        db.deserialize_read_exact("main", plaintext.as_slice(), plaintext.len(), true)
            .map_err(|_| CensusError::Storage)?;
        let (version, account, snapshot): (u32, String, String) = db
            .query_row(
                "SELECT version, account, snapshot FROM census WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|_| CensusError::Storage)?;
        let snapshot = Zeroizing::new(snapshot);
        if version != 1 {
            return Err(CensusError::UnsupportedVersion);
        }
        if account != self.keys.account.as_str() {
            return Err(CensusError::AccountMismatch);
        }
        let mut census: Census =
            serde_json::from_str(&snapshot).map_err(|_| CensusError::Storage)?;
        if census.account != self.keys.account {
            return Err(CensusError::AccountMismatch);
        }
        // An interrupted scan can resume, but reopening cannot imply a running worker.
        if census.acquisition_state == super::ScanState::Running {
            census.acquisition_state = super::ScanState::Cancelled;
        }
        for folder in census.folders.values_mut() {
            if folder.state == super::ScanState::Running {
                folder.state = super::ScanState::Cancelled;
            }
        }
        Ok(census)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn interruption_before_atomic_replacement_retains_the_previous_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let account = AccountId::new("interruption-test").unwrap();
        let store = EncryptedStore::new(
            dir.path().to_owned(),
            CensusKeys::derive(&LocalSecret::random(), &account),
        )
        .unwrap();
        let original = Census::new(account);
        store.save(&original).unwrap();
        let before = fs::read(store.path()).unwrap();
        let mut next = original.clone();
        next.reconcile_folder("private-folder-name", 1, BTreeSet::from([1]))
            .unwrap();
        let staged = store.stage(&next).unwrap();
        let staged_bytes = fs::read(staged.path()).unwrap();
        assert!(
            !staged_bytes
                .windows(19)
                .any(|w| w == b"private-folder-name")
        );
        let (file, orphan) = staged.keep().unwrap();
        drop(file);
        assert_eq!(fs::read(store.path()).unwrap(), before);
        assert_eq!(store.load().unwrap(), original);
        // A crash can also leave an incomplete encrypted staging file.
        fs::write(&orphan, &staged_bytes[..20]).unwrap();
        assert_eq!(store.load().unwrap(), original);
        store.save(&next).unwrap();
        assert_eq!(
            store.load().unwrap().folders["private-folder-name"].pending(10),
            vec![1]
        );
    }
}

fn memory_database() -> Result<Connection, CensusError> {
    let db = Connection::open_in_memory().map_err(|_| CensusError::Storage)?;
    db.execute_batch(
        "PRAGMA temp_store=MEMORY; PRAGMA journal_mode=MEMORY; PRAGMA secure_delete=ON;",
    )
    .map_err(|_| CensusError::Storage)?;
    Ok(db)
}
