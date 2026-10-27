use super::*;
use crate::file_lock::ExclusiveFileLock;
use crate::ledger::{InstalledLedger, file::LockedLedger};

const MAX_ACCOUNTS: usize = 8;
pub(crate) const SELECTED_LIMITATIONS: &str = "Only the explicitly selected captured local accounts and approved authoritative ledger are restored. Provider passwords, uncaptured changes and external mailbox/send/unsubscribe effects are excluded. Current owner ledger edits reject replacement. All restored accounts remain read-only pending later authoritative provider reconciliation. No blind retry, compensation or daily readiness.";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum LedgerSelection {
    Excluded {
        source_version: Option<String>,
    },
    Included {
        inventory: Inventory,
        schema: u32,
        entries: usize,
        semantics: String,
    },
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SelectedManifest {
    pub version: u32,
    pub captured_at: u64,
    pub accounts: Vec<Manifest>,
    pub excluded_accounts: Vec<AccountId>,
    pub ledger: LedgerSelection,
    pub limitations: String,
}
struct SelectedCaptured {
    manifest: SelectedManifest,
    accounts: Vec<Captured>,
    ledger: Option<SecretBytes>,
}
/// A selected capture inside a version-2 JWE bundle, files inline.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacySelected {
    manifest: SelectedManifest,
    accounts: Vec<LegacyCaptured>,
    ledger: Option<SecretBytes>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum LedgerTarget {
    PreserveCurrent { version: Option<String> },
    Absent,
    Identical { version: String },
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SelectedRestorePreview {
    pub manifest: SelectedManifest,
    pub bundle: String,
    pub target: String,
    pub accounts_absent: bool,
    pub ledger_target: LedgerTarget,
    pub consequences: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectedTransaction {
    version: u32,
    authority: String,
    preview: SelectedRestorePreview,
}
fn ids(manifest: &SelectedManifest) -> Vec<AccountId> {
    manifest
        .accounts
        .iter()
        .map(|a| a.account.clone())
        .collect()
}
fn selection(ids: &[AccountId]) -> Result<Vec<AccountId>, AcquisitionError> {
    if ids.is_empty() || ids.len() > MAX_ACCOUNTS {
        return Err(AcquisitionError::InvalidInput);
    }
    let mut sorted = ids.to_vec();
    sorted.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    if sorted
        .iter()
        .map(|id| id.as_str().to_ascii_lowercase())
        .collect::<BTreeSet<_>>()
        .len()
        != sorted.len()
    {
        return Err(AcquisitionError::InvalidInput);
    }
    Ok(sorted)
}
fn path_present(path: &Path) -> Result<bool, AcquisitionError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(AcquisitionError::Storage),
    }
}
fn ledger_inventory(bytes: &[u8]) -> Result<LedgerSelection, AcquisitionError> {
    let doc = crate::ledger::file::decode_recovery_bytes(bytes)
        .map_err(|_| AcquisitionError::InvalidInput)?;
    Ok(LedgerSelection::Included {
        inventory: Inventory {
            resource: "authoritative_addresses_yaml".into(),
            bytes: bytes.len() as u64,
            sha256: digest(bytes),
        },
        schema: doc.version,
        entries: doc.entries.len(),
        semantics: crate::ledger::SEMANTICS.into(),
    })
}
fn ledger_lock(
    ledger: Option<&InstalledLedger>,
) -> Result<Option<LockedLedger<'_>>, AcquisitionError> {
    ledger
        .map(|l| l.lock().map_err(|_| AcquisitionError::Storage))
        .transpose()
}
fn raw_ledger(guard: Option<&LockedLedger<'_>>) -> Result<Option<Vec<u8>>, AcquisitionError> {
    guard
        .map(|g| g.raw().map_err(|_| AcquisitionError::Storage))
        .transpose()
        .map(Option::flatten)
}
/// A single-account capture as a selection of one, with no ledger.
fn single_as_selected(captured: Captured) -> SelectedCaptured {
    SelectedCaptured {
        manifest: SelectedManifest {
            version: 2,
            captured_at: captured.manifest.captured_at,
            accounts: vec![captured.manifest.clone()],
            excluded_accounts: vec![],
            ledger: LedgerSelection::Excluded {
                source_version: None,
            },
            limitations: SELECTED_LIMITATIONS.into(),
        },
        accounts: vec![captured],
        ledger: None,
    }
}
/// Opens a bundle of any version, single or selected, staging its account
/// files, and returns the authenticated selection and bundle identity.
fn selected_open(
    bundle: &Path,
    secret: &SecretValue,
) -> Result<(SelectedCaptured, String), AcquisitionError> {
    let (captured, id) = if stream::Reader::is_stream(bundle)? {
        let (mut reader, body) = stream::Reader::open::<Body>(bundle, secret, MAX_STREAM)?;
        let captured = match body {
            Body::Single { captured } => {
                let captured = stage_streamed(captured, &mut reader)?;
                validate_captured(&captured)?;
                single_as_selected(captured)
            }
            Body::Selected {
                manifest,
                accounts,
                ledger,
            } => {
                if accounts.len() > MAX_ACCOUNTS {
                    return Err(AcquisitionError::InvalidInput);
                }
                SelectedCaptured {
                    manifest,
                    accounts: accounts
                        .into_iter()
                        .map(|a| stage_streamed(a, &mut reader))
                        .collect::<Result<_, _>>()?,
                    ledger,
                }
            }
        };
        (captured, reader.finish()?)
    } else {
        let bytes = read_bounded(bundle, MAX_BUNDLE)?;
        let plain = open_plain(&bytes, secret)?;
        let captured = match serde_json::from_slice::<LegacySelected>(&plain) {
            Ok(c) => SelectedCaptured {
                manifest: c.manifest,
                accounts: c
                    .accounts
                    .into_iter()
                    .map(stage_legacy)
                    .collect::<Result<_, _>>()?,
                ledger: c.ledger,
            },
            Err(_) => {
                let legacy: LegacyCaptured =
                    serde_json::from_slice(&plain).map_err(|_| AcquisitionError::InvalidInput)?;
                let legacy = stage_legacy(legacy)?;
                validate_captured(&legacy)?;
                single_as_selected(legacy)
            }
        };
        (captured, digest(&bytes))
    };
    validate_selected(&captured)?;
    Ok((captured, id))
}
fn validate_selected(c: &SelectedCaptured) -> Result<(), AcquisitionError> {
    let m = &c.manifest;
    let selected = selection(&ids(m))?;
    if m.version != 2
        || m.limitations != SELECTED_LIMITATIONS
        || ids(m) != selected
        || c.accounts.len() != selected.len()
        || m.excluded_accounts.len() > 64
        || m.excluded_accounts
            .windows(2)
            .any(|w| w[0].as_str() >= w[1].as_str())
        || m.excluded_accounts.iter().any(|a| selected.contains(a))
    {
        return Err(AcquisitionError::InvalidInput);
    }
    let mut logins = BTreeSet::new();
    for (captured, manifest) in c.accounts.iter().zip(&m.accounts) {
        if captured.manifest != *manifest
            || manifest.captured_at != m.captured_at
            || !logins.insert((
                format!("{:?}", captured.account.provider),
                captured.account.login.trim().to_lowercase(),
            ))
        {
            return Err(AcquisitionError::InvalidInput);
        }
        validate_captured(captured)?;
    }
    match (&m.ledger, &c.ledger) {
        (LedgerSelection::Excluded { source_version }, None)
            if source_version.as_deref().is_none_or(valid_digest) =>
        {
            ()
        }
        (expected @ LedgerSelection::Included { .. }, Some(bytes))
            if *expected == ledger_inventory(&bytes.0)? =>
        {
            ()
        }
        _ => return Err(AcquisitionError::InvalidInput),
    }
    Ok(())
}
fn selected_plan(
    operation: &str,
    value: &impl Serialize,
    accounts: &[AccountId],
    receipt: String,
) -> Result<crate::confirmation::Plan, AcquisitionError> {
    Ok(crate::confirmation::Plan {
        actions: accounts
            .iter()
            .map(|id| crate::confirmation::ExactAction::Recovery {
                account: id.as_str().into(),
                operation: operation.into(),
                manifest_digest: digest(&encoded(value).expect("serializable recovery model")),
                consequences: SELECTED_LIMITATIONS.into(),
            })
            .collect(),
        exact_receipt: receipt,
    })
}
pub(crate) fn selected_restore_plan(
    p: &SelectedRestorePreview,
) -> Result<crate::confirmation::Plan, AcquisitionError> {
    selected_plan(
        "restore_selected_absent_accounts",
        p,
        &ids(&p.manifest),
        digest(&encoded(p)?),
    )
}
impl Accounts {
    fn selected_locks(
        &self,
        selected: &[AccountId],
    ) -> Result<Vec<ExclusiveFileLock>, AcquisitionError> {
        selection(selected)?
            .iter()
            .map(|id| self.lock_account(id))
            .collect()
    }
    fn selected_snapshot(
        &self,
        selected: &[AccountId],
        captured_at: u64,
        include_ledger: bool,
        ledger: Option<&InstalledLedger>,
    ) -> Result<(SelectedCaptured, Vec<EncryptedStore>), AcquisitionError> {
        let mut accounts = vec![];
        let mut stores = vec![];
        for id in selection(selected)? {
            let (captured, store) = self.capture_locked(&id, captured_at)?;
            accounts.push(captured);
            stores.push(store);
        }
        let guard = ledger_lock(ledger)?;
        let raw = raw_ledger(guard.as_ref())?;
        let authority = if include_ledger {
            Some(SecretBytes(Zeroizing::new(
                raw.ok_or(AcquisitionError::InvalidInput)?,
            )))
        } else {
            None
        };
        let ledger = match &authority {
            Some(bytes) => ledger_inventory(&bytes.0)?,
            None => LedgerSelection::Excluded {
                source_version: raw_ledger(guard.as_ref())?.as_deref().map(digest),
            },
        };
        let mut excluded_accounts: Vec<_> = self
            .list()?
            .into_iter()
            .map(|a| a.id)
            .filter(|a| !selected.contains(a))
            .collect();
        excluded_accounts.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        let captured = SelectedCaptured {
            manifest: SelectedManifest {
                version: 2,
                captured_at,
                accounts: accounts.iter().map(|c| c.manifest.clone()).collect(),
                excluded_accounts,
                ledger,
                limitations: SELECTED_LIMITATIONS.into(),
            },
            accounts,
            ledger: authority,
        };
        validate_selected(&captured)?;
        Ok((captured, stores))
    }
    pub(crate) fn selected_capture_preview(
        &self,
        selected: &[AccountId],
        include_ledger: bool,
        ledger: Option<&InstalledLedger>,
        captured_at: u64,
    ) -> Result<(SelectedManifest, crate::confirmation::Plan), AcquisitionError> {
        let selected = selection(selected)?;
        let _leases = self.selected_locks(&selected)?;
        let (captured, stores) =
            self.selected_snapshot(&selected, captured_at, include_ledger, ledger)?;
        let bindings: Vec<_> = stores
            .iter()
            .map(|s| {
                s.keys().binding_for(
                    b"mailroom/recovery-selected-capture/v2",
                    &encoded(&captured.manifest).expect("serializable manifest"),
                )
            })
            .collect();
        let plan = selected_plan(
            "capture_selected",
            &captured.manifest,
            &selected,
            digest(&encoded(&bindings)?),
        )?;
        Ok((captured.manifest, plan))
    }
    pub(crate) fn selected_capture(
        &self,
        preview: &SelectedManifest,
        secret: &SecretValue,
        directory: &Path,
        ledger: Option<&InstalledLedger>,
        authorize: impl FnOnce(crate::confirmation::Plan) -> Result<(), AcquisitionError>,
    ) -> Result<String, AcquisitionError> {
        let selected = selection(&ids(preview))?;
        let _leases = self.selected_locks(&selected)?;
        let (captured, stores) = self.selected_snapshot(
            &selected,
            preview.captured_at,
            matches!(preview.ledger, LedgerSelection::Included { .. }),
            ledger,
        )?;
        if captured.manifest != *preview {
            return Err(AcquisitionError::InvalidInput);
        }
        let bindings: Vec<_> = stores
            .iter()
            .map(|s| {
                s.keys().binding_for(
                    b"mailroom/recovery-selected-capture/v2",
                    &encoded(preview).expect("serializable manifest"),
                )
            })
            .collect();
        authorize(selected_plan(
            "capture_selected",
            preview,
            &selected,
            digest(&encoded(&bindings)?),
        )?)?;
        let body = Body::Selected {
            manifest: captured.manifest.clone(),
            accounts: captured.accounts.iter().map(Captured::header).collect(),
            ledger: captured.ledger.clone(),
        };
        let accounts: Vec<_> = captured.accounts.iter().collect();
        let id = write_bundle(
            directory,
            ".selected-stage-",
            secret,
            &body,
            &accounts,
            "selected_capture_staged",
            |staged| {
                let (_, id) = selected_open(staged, secret)?;
                // Recheck the installed authority under its guard at the final publication boundary.
                let guard = ledger_lock(ledger)?;
                let raw = raw_ledger(guard.as_ref())?;
                let current = match &captured.ledger {
                    Some(_) => {
                        ledger_inventory(raw.as_deref().ok_or(AcquisitionError::InvalidInput)?)?
                    }
                    None => LedgerSelection::Excluded {
                        source_version: raw.as_deref().map(digest),
                    },
                };
                if current != preview.ledger {
                    return Err(AcquisitionError::InvalidInput);
                }
                Ok(id)
            },
        )?;
        boundary("selected_capture_published")?;
        boundary("selected_capture_before_ack")?;
        Ok(id)
    }
    fn selected_marker(&self) -> PathBuf {
        self.paths.config.join("recovery-selected.restore")
    }
    fn selected_target(
        &self,
        captured: &SelectedCaptured,
        restore_ledger: bool,
        guard: Option<&LockedLedger<'_>>,
    ) -> Result<LedgerTarget, AcquisitionError> {
        if !restore_ledger {
            return Ok(LedgerTarget::PreserveCurrent {
                version: raw_ledger(guard)?.as_deref().map(digest),
            });
        }
        let bytes = &captured
            .ledger
            .as_ref()
            .ok_or(AcquisitionError::InvalidInput)?
            .0;
        if guard.is_none() {
            return Err(AcquisitionError::InvalidInput);
        }
        match raw_ledger(guard)? {
            None => Ok(LedgerTarget::Absent),
            Some(current) if current == **bytes => Ok(LedgerTarget::Identical {
                version: digest(&current),
            }),
            _ => Err(AcquisitionError::InvalidInput),
        }
    }
    fn selected_preview(
        &self,
        captured: &SelectedCaptured,
        bundle: &str,
        target: LedgerTarget,
    ) -> SelectedRestorePreview {
        SelectedRestorePreview {
            manifest: captured.manifest.clone(),
            bundle: bundle.into(),
            target: self.paths.config.to_string_lossy().into_owned(),
            accounts_absent: true,
            ledger_target: target,
            consequences: SELECTED_LIMITATIONS.into(),
        }
    }
    fn selected_validate_accounts(&self, c: &SelectedCaptured) -> Result<(), AcquisitionError> {
        for captured in &c.accounts {
            self.validate_account(&captured.account, &captured.manifest.account)?;
        }
        Ok(())
    }
    fn selected_absent(&self, a: &Account) -> Result<(), AcquisitionError> {
        self.absent(a)?;
        if path_present(&self.metadata_path(&a.id))? || !self.selected_cipher_paths(a)?.is_empty() {
            return Err(AcquisitionError::InvalidInput);
        }
        Ok(())
    }
    fn selected_cipher_paths(&self, a: &Account) -> Result<Vec<PathBuf>, AcquisitionError> {
        if !self
            .paths
            .data
            .try_exists()
            .map_err(|_| AcquisitionError::Storage)?
        {
            return Ok(vec![]);
        }
        let mut paths = vec![];
        for entry in fs::read_dir(&self.paths.data).map_err(|_| AcquisitionError::Storage)? {
            let path = entry.map_err(|_| AcquisitionError::Storage)?.path();
            if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with(&format!("{}.", a.id.as_str())) && n.ends_with(".census.enc")
            }) {
                paths.push(path);
            }
        }
        Ok(paths)
    }
    pub(crate) fn selected_restore_preview(
        &self,
        bundle: &Path,
        secret: &SecretValue,
        restore_ledger: bool,
        ledger: Option<&InstalledLedger>,
    ) -> Result<SelectedRestorePreview, AcquisitionError> {
        let (captured, id) = selected_open(bundle, secret)?;
        self.selected_validate_accounts(&captured)?;
        let _installation = self.lock_installation()?;
        let _leases = self.selected_locks(&ids(&captured.manifest))?;
        for c in &captured.accounts {
            self.selected_absent(&c.account)?;
        }
        let guard = ledger_lock(ledger)?;
        let target = self.selected_target(&captured, restore_ledger, guard.as_ref())?;
        Ok(self.selected_preview(&captured, &id, target))
    }
    pub(crate) fn selected_restore(
        &self,
        bundle: &Path,
        secret: &SecretValue,
        preview: &SelectedRestorePreview,
        ledger: Option<&InstalledLedger>,
        authorize: impl FnOnce(crate::confirmation::Plan) -> Result<(), AcquisitionError>,
    ) -> Result<Vec<Restored>, AcquisitionError> {
        let (captured, id) = selected_open(bundle, secret)?;
        self.selected_validate_accounts(&captured)?;
        let _installation = self.lock_installation()?;
        let _leases = self.selected_locks(&ids(&captured.manifest))?;
        for c in &captured.accounts {
            self.selected_absent(&c.account)?;
        }
        let guard = ledger_lock(ledger)?;
        let target = self.selected_target(
            &captured,
            !matches!(preview.ledger_target, LedgerTarget::PreserveCurrent { .. }),
            guard.as_ref(),
        )?;
        if self.selected_preview(&captured, &id, target) != *preview {
            return Err(AcquisitionError::InvalidInput);
        }
        authorize(selected_restore_plan(preview)?)?;
        let tx = SelectedTransaction {
            version: 2,
            authority: nonce(),
            preview: preview.clone(),
        };
        self.stage_bundle(bundle, &preview.bundle)?;
        write_json(&self.selected_marker(), &tx)?;
        boundary("selected_restore_intent")?;
        self.finish_selected(captured, &tx, guard.as_ref())
    }
    pub(crate) fn selected_resume(
        &self,
        secret: &SecretValue,
        ledger: Option<&InstalledLedger>,
    ) -> Result<Vec<Restored>, AcquisitionError> {
        let _installation = self.lock_installation()?;
        let tx: SelectedTransaction =
            serde_json::from_slice(&read_bounded(&self.selected_marker(), 256 * 1024)?)
                .map_err(|_| AcquisitionError::Storage)?;
        if tx.version != 2
            || !valid_digest(&tx.preview.bundle)
            || tx.authority.len() != 32
            || tx.authority.bytes().any(|b| !b.is_ascii_hexdigit())
            || tx.preview.target != self.paths.config.to_string_lossy()
        {
            return Err(AcquisitionError::InvalidInput);
        }
        let (captured, bundle) = selected_open(&self.staged_bundle(&tx.preview.bundle)?, secret)?;
        if bundle != tx.preview.bundle {
            return Err(AcquisitionError::Authentication);
        }
        if captured.manifest != tx.preview.manifest
            || !tx.preview.accounts_absent
            || tx.preview.consequences != SELECTED_LIMITATIONS
        {
            return Err(AcquisitionError::InvalidInput);
        }
        self.selected_validate_accounts(&captured)?;
        let _leases = self.selected_locks(&ids(&captured.manifest))?;
        let guard = ledger_lock(ledger)?;
        self.finish_selected(captured, &tx, guard.as_ref())
    }
    fn selected_restored(c: &Captured, tx: &SelectedTransaction) -> Restored {
        Restored {
            authority: tx.authority.clone(),
            bundle: tx.preview.bundle.clone(),
            captured_at: tx.preview.manifest.captured_at,
            authenticated: true,
            reconciliation_required: true,
            resources: c.manifest.resources.clone(),
            key_material: c.manifest.key_material.clone(),
        }
    }
    fn finish_selected(
        &self,
        mut captured: SelectedCaptured,
        tx: &SelectedTransaction,
        guard: Option<&LockedLedger<'_>>,
    ) -> Result<Vec<Restored>, AcquisitionError> {
        let restore_ledger = !matches!(
            tx.preview.ledger_target,
            LedgerTarget::PreserveCurrent { .. }
        );
        if restore_ledger {
            self.selected_target(&captured, true, guard)?;
        }
        let mut restored = vec![];
        // Preflight every account and credential before publishing any selected resource, including on restart.
        for c in &mut captured.accounts {
            let r = Self::selected_restored(c, tx);
            c.account.connection = ConnectionState::Unverified;
            c.account.restored = Some(r.clone());
            if self
                .paths
                .config
                .join("lifecycle")
                .join(format!("{}.json", c.account.id.as_str()))
                .exists()
                || self.restore_marker(&c.account.id).exists()
            {
                return Err(AcquisitionError::LifecyclePending);
            }
            if path_present(&self.metadata_path(&c.account.id))?
                && encoded(&self.load_metadata(&c.account.id)?)?.as_slice()
                    != encoded(&c.account)?.as_slice()
            {
                return Err(AcquisitionError::InvalidInput);
            }
            if self.list()?.iter().any(|a| {
                a.id != c.account.id
                    && a.provider == c.account.provider
                    && a.login.trim().eq_ignore_ascii_case(c.account.login.trim())
            }) {
                return Err(AcquisitionError::InvalidInput);
            }
            let path = cipher_path(&self.paths.data, &c.account.id, c.account.generation);
            if self
                .selected_cipher_paths(&c.account)?
                .iter()
                .any(|p| p != &path)
            {
                return Err(AcquisitionError::InvalidInput);
            }
            if path_present(&path)? && !c.resource.matches(&path, &c.manifest)? {
                return Err(AcquisitionError::InvalidInput);
            }
            for (reference, expected) in [
                Some((&c.account.local_secret_ref, c.key.0.as_slice())),
                c.account
                    .identity_ref
                    .as_ref()
                    .zip(c.identity.as_ref())
                    .map(|(r, b)| (r, b.0.as_slice())),
            ]
            .into_iter()
            .flatten()
            {
                match self.backend.get(reference) {
                    Ok(v) if v.bytes() == expected => (),
                    Err(AcquisitionError::CredentialMissing) => (),
                    _ => return Err(AcquisitionError::Credentials),
                }
            }
            if !matches!(
                self.backend.get(&c.account.password_ref),
                Err(AcquisitionError::CredentialMissing)
            ) {
                return Err(AcquisitionError::Credentials);
            }
            restored.push(r);
        }
        // Hold the full set of stable store locks through the single global commit.
        let _stores: Vec<_> = captured
            .accounts
            .iter()
            .map(|c| EncryptedStore::lock_directory(&self.paths.data, &c.account.id))
            .collect::<Result<_, _>>()?;
        if restore_ledger {
            let bytes = &captured
                .ledger
                .as_ref()
                .ok_or(AcquisitionError::InvalidInput)?
                .0;
            let expected = match &tx.preview.ledger_target {
                LedgerTarget::Absent => None,
                LedgerTarget::Identical { version } => Some(version.as_str()),
                LedgerTarget::PreserveCurrent { .. } => unreachable!(),
            };
            guard
                .ok_or(AcquisitionError::InvalidInput)?
                .restore_exact(expected, bytes)
                .map_err(|_| AcquisitionError::Storage)?;
        }
        boundary("selected_restore_ledger")?;
        for (index, c) in captured.accounts.into_iter().enumerate() {
            let a = c.account;
            self.put_exact(&a.local_secret_ref, &c.key.0)?;
            if let (Some(reference), Some(identity)) = (&a.identity_ref, &c.identity) {
                self.put_exact(reference, &identity.0)?;
            }
            boundary("selected_restore_credentials")?;
            if index == 1 {
                boundary("selected_restore_second_credentials")?;
            }
            let path = cipher_path(&self.paths.data, &a.id, a.generation);
            if !path.exists() {
                c.resource.publish(&path)?;
            }
            boundary("selected_restore_data")?;
            if index == 1 {
                boundary("selected_restore_second_data")?;
            }
            if !self.metadata_path(&a.id).exists() {
                publish_new(&self.metadata_path(&a.id), &encoded(&a)?)?;
            }
            boundary("selected_restore_metadata")?;
            if index == 1 {
                boundary("selected_restore_second_metadata")?;
            }
            if index == 0 {
                boundary("selected_restore_first_account")?;
            }
        }
        remove_file(&self.selected_marker())?;
        boundary("selected_restore_committed")?;
        Ok(restored)
    }
    pub(crate) fn selected_pending(&self) -> Result<Vec<AccountId>, AcquisitionError> {
        if !self
            .selected_marker()
            .try_exists()
            .map_err(|_| AcquisitionError::Storage)?
        {
            return Ok(vec![]);
        }
        let tx: SelectedTransaction =
            serde_json::from_slice(&read_bounded(&self.selected_marker(), 256 * 1024)?)
                .map_err(|_| AcquisitionError::Storage)?;
        Ok(ids(&tx.preview.manifest))
    }
}

#[cfg(test)]
mod tests;
