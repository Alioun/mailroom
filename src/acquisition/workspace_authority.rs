use super::{Account, Accounts, AcquisitionError, Provider};
use crate::{
    census::{AccountId, EncryptedStore},
    rules::{EngineContext, EvidenceSource, workspace::WorkspaceContext},
};
use serde::Serialize;

#[derive(Serialize)]
pub(super) struct SyntheticWorkspaceAuthority {
    account: AccountId,
    provider: Provider,
    identity: String,
    epoch: String,
    profile: &'static str,
    recipient_evidence: EvidenceSource,
    capabilities: std::collections::BTreeSet<String>,
}
impl Accounts {
    pub(crate) fn workspace_context(
        &self,
        account: &Account,
        store: &EncryptedStore,
    ) -> Result<WorkspaceContext, AcquisitionError> {
        let local = WorkspaceContext::local(account.provider);
        let mut engine = EngineContext::local(
            account.id.as_str().into(),
            match account.provider {
                Provider::MailboxOrg => crate::rules::Engine::MailboxSieve,
                Provider::Gmail => crate::rules::Engine::GmailBacklog,
            },
        );
        let authority = self.workspace_authority.as_ref().filter(|a| {
            a.account == account.id && a.provider == account.provider && !store.recovery_pending()
        });
        if let Some(a) = authority {
            engine.recipient_evidence = match &a.recipient_evidence {
                EvidenceSource::SyntheticEnvelope { source }
                    if a.profile == "synthetic-envelope/v1"
                        && source.validate()
                        && source.account == account.id.as_str()
                        && source.provider
                            == match account.provider {
                                Provider::MailboxOrg => "mailbox_org",
                                Provider::Gmail => "gmail",
                            }
                        && source.identity == a.identity
                        && source.profile == a.profile
                        && source.epoch == a.epoch =>
                {
                    let mut source = source.clone();
                    source.authority = store.keys().binding_for(
                        b"mailroom/delivery-authority/v1",
                        &serde_json::to_vec(&(
                            self.sieve_authority(account, store)?,
                            &account.login,
                            a,
                            self.folder_source.as_ref().map(|s| s.identity()),
                            self.sieve_source.as_ref().map(|s| s.identity()),
                        ))
                        .map_err(|_| AcquisitionError::Storage)?,
                    );
                    EvidenceSource::SyntheticEnvelope { source }
                }
                EvidenceSource::SyntheticEnvelope { .. } => EvidenceSource::Unknown,
                old => old.clone(),
            };
            engine.capabilities = a.capabilities.clone();
        } else if self.live_transports && !store.recovery_pending() {
            if let Ok(state) = store.load_state() {
                if let Some(baseline) = &state.sieve.last_successful {
                    if let Some(exts) = baseline.capabilities.get("SIEVE") {
                        for ext in exts.split_whitespace() {
                            engine.capabilities.insert(ext.to_ascii_lowercase());
                        }
                    }
                }
            }
        }
        let bytes = serde_json::to_vec(&(
            self.sieve_authority(account, store)?,
            &account.login,
            authority,
            self.folder_source.as_ref().map(|s| s.identity()),
            self.sieve_source.as_ref().map(|s| s.identity()),
            &engine,
            local.binding(),
        ))
        .map_err(|_| AcquisitionError::Storage)?;
        let binding = store
            .keys()
            .binding_for(b"mailroom/workspace-authority/v1", &bytes);
        Ok(WorkspaceContext::bound(
            account.id.clone(),
            account.provider,
            engine,
            binding,
        ))
    }
    #[cfg(test)]
    pub(crate) fn install_synthetic_envelope(&mut self, account: &Account, epoch: &str) {
        self.install_synthetic_workspace(account, epoch);
        let authority = self.workspace_authority.as_mut().unwrap();
        authority.identity = "explicit-synthetic-smtp-delivery".into();
        authority.profile = "synthetic-envelope/v1";
        authority.capabilities.insert("envelope".into());
        authority.recipient_evidence = EvidenceSource::SyntheticEnvelope {
            source: crate::census::delivery::Source {
                account: account.id.as_str().into(),
                provider: match account.provider {
                    Provider::MailboxOrg => "mailbox_org",
                    Provider::Gmail => "gmail",
                }
                .into(),
                identity: authority.identity.clone(),
                profile: authority.profile.into(),
                epoch: epoch.into(),
                authority: "installed-only".into(),
            },
        };
    }

    #[cfg(test)]
    pub(crate) fn install_synthetic_workspace(&mut self, account: &Account, epoch: &str) {
        assert!(!epoch.is_empty());
        self.workspace_authority = Some(SyntheticWorkspaceAuthority {
            account: account.id.clone(),
            provider: account.provider,
            identity: "explicit-synthetic-delivered-to".into(),
            epoch: epoch.into(),
            profile: "synthetic-workspace/v1",
            recipient_evidence: EvidenceSource::SyntheticDeliveredTo,
            capabilities: [
                "fileinto",
                "copy",
                "duplicate",
                "relational",
                "comparator-i;ascii-numeric",
                "imap4flags",
            ]
            .map(String::from)
            .into(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        acquisition::{Cancellation, SecretValue, folder_provisioning},
        rules::{
            proof::{Baseline, Destination},
            workspace,
        },
    };

    #[test]
    fn explicit_account_authority_is_shared_and_never_recovered_from_exports() {
        let root = tempfile::tempdir().unwrap();
        let (mut repo, account, _fake) = folder_provisioning::tests::install(root.path());
        let store = repo.store(&account).unwrap();
        let mut state = store.load_state().unwrap();
        let local = repo.workspace_context(&account, &store).unwrap();
        let conservative = workspace::preview(&state, &local, store.keys()).unwrap();
        assert!(conservative.proof.artifact.folders().is_empty());
        repo.install_synthetic_workspace(&account, "first-installation");
        let installed = repo.workspace_context(&account, &store).unwrap();
        let proof = workspace::preview(&state, &installed, store.keys()).unwrap();
        assert!(!proof.proof.artifact.folders().is_empty());
        assert!(
            workspace::confirm_export(&mut state, &installed, store.keys(), &conservative.receipt)
                .is_err()
        );
        workspace::confirm_export(&mut state, &installed, store.keys(), &proof.receipt).unwrap();
        let current = workspace::preview(&state, &installed, store.keys()).unwrap();
        let plan = repo
            .folder_preview(&account, &store, &state, Cancellation::default())
            .unwrap();
        assert_eq!(
            &plan.operation.plan.logical_leaves,
            current.proof.artifact.folders()
        );
        assert_eq!(plan.operation.artifact, current.proof.artifact.identity());
        assert_eq!(plan.operation.proof_binding, current.receipt);
        assert!(
            workspace::view(&state, &installed)
                .exports
                .last()
                .unwrap()
                .usable
        );
        let before = serde_json::to_value(&state).unwrap();
        for changed in [
            "epoch",
            "recipient",
            "capability",
            "removed",
            "wrong-account",
            "provider",
            "profile",
        ] {
            repo.install_synthetic_workspace(&account, "first-installation");
            match changed {
                "epoch" => repo.workspace_authority.as_mut().unwrap().epoch.push('2'),
                "recipient" => {
                    repo.workspace_authority
                        .as_mut()
                        .unwrap()
                        .recipient_evidence = EvidenceSource::SyntheticOriginalTo
                }
                "capability" => {
                    repo.workspace_authority
                        .as_mut()
                        .unwrap()
                        .capabilities
                        .remove("copy");
                }
                "removed" => repo.workspace_authority = None,
                "wrong-account" => {
                    repo.workspace_authority.as_mut().unwrap().account =
                        AccountId::new("other-account").unwrap()
                }
                "provider" => repo.workspace_authority.as_mut().unwrap().provider = Provider::Gmail,
                "profile" => {
                    repo.workspace_authority.as_mut().unwrap().profile = "synthetic-workspace/v2"
                }
                _ => unreachable!(),
            }
            let changed_context = repo.workspace_context(&account, &store).unwrap();
            assert!(
                !workspace::view(&state, &changed_context)
                    .exports
                    .last()
                    .unwrap()
                    .usable
            );
            assert!(
                workspace::confirm_export(
                    &mut state,
                    &changed_context,
                    store.keys(),
                    &current.receipt
                )
                .is_err()
            );
            assert!(
                repo.folder_confirm(
                    &account,
                    &store,
                    &mut state,
                    &plan.receipt,
                    Cancellation::default(),
                    |_| Ok(())
                )
                .is_err()
            );
            let mut previous = state.clone();
            previous.workspace.as_mut().unwrap().input.baseline = Baseline::Previous;
            assert!(workspace::preview(&previous, &changed_context, store.keys()).is_err());
            assert_eq!(serde_json::to_value(&state).unwrap(), before);
        }
        repo.workspace_authority = None;
        assert!(
            workspace::preview(
                &state,
                &repo.workspace_context(&account, &store).unwrap(),
                store.keys()
            )
            .unwrap()
            .proof
            .artifact
            .folders()
            .is_empty()
        );
        let external = repo
            .fixture_sieve_store(&account.id, root.path())
            .unwrap()
            .load_state()
            .unwrap();
        assert!(
            !external
                .synthetic_folders
                .unwrap()
                .commands
                .contains(&"CREATE".into())
        );
        drop(store);
        let other = repo
            .create(
                Provider::MailboxOrg,
                "other@example.test".into(),
                &SecretValue::new(b"invented".to_vec()),
            )
            .unwrap();
        let store = repo.store(&other).unwrap();
        let state = store.load_state().unwrap();
        assert!(
            workspace::validate(
                &crate::rules::workspace::WorkspaceInput {
                    documents: serde_json::from_str::<crate::engine::PreviewInput>(include_str!(
                        "../../examples/sample.json"
                    ))
                    .unwrap()
                    .documents,
                    pins: vec![],
                    as_of: 0,
                    baseline: Baseline::Current
                },
                &state,
                &installed
            )
            .is_err()
        );
    }

    #[test]
    fn installed_fixture_evidence_retains_unknown_and_mixed_roles_without_side_effects() {
        let root = tempfile::tempdir().unwrap();
        let (mut repo, account, _fake) = folder_provisioning::tests::install(root.path());
        repo.install_synthetic_workspace(&account, "explicit-only");
        let store = repo.store(&account).unwrap();
        let mut state = store.load_state().unwrap();
        let mut sample: crate::engine::PreviewInput =
            serde_json::from_str(include_str!("../../examples/sample.json")).unwrap();
        sample.context.account = account.id.as_str().into();
        state.census = crate::engine::synthetic_census(&sample).unwrap().into();
        state.workspace = Some(workspace::Workspace {
            revision: 1,
            input: workspace::WorkspaceInput {
                documents: sample.documents,
                pins: vec![],
                as_of: sample.as_of,
                baseline: Baseline::Current,
            },
        });
        let context = repo.workspace_context(&account, &store).unwrap();
        let proof = workspace::preview(&state, &context, store.keys()).unwrap();
        assert!(proof.proof.counts.filed > 0);
        let held: Vec<_> = proof
            .proof
            .decisions
            .iter()
            .filter(|d| d.subject.contains("unknown role") || d.subject.contains("mixed roles"))
            .collect();
        assert_eq!(held.len(), 2);
        for decision in held {
            assert_eq!(decision.destination, Destination::Inbox);
            assert!(decision.review);
            assert!(decision.copies.is_empty());
            assert!(!decision.mark_read);
        }
        repo.workspace_authority = None;
        let local = workspace::preview(
            &state,
            &repo.workspace_context(&account, &store).unwrap(),
            store.keys(),
        )
        .unwrap();
        assert!(local.proof.artifact.folders().is_empty());
        assert_eq!(local.proof.counts.copies, 0);
        assert_eq!(local.proof.counts.filed, 0);
    }
}

#[cfg(test)]
#[path = "workspace_authority/envelope_tests.rs"]
pub(crate) mod envelope_tests;
