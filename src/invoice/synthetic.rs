use super::*;
use crate::census::{Direction, EncryptedStore, ScanState};
use std::path::Path;

pub(crate) fn initialize_synthetic(state: &mut AccountState) -> bool {
    if !state.invoices.sources.is_empty() || state.census.acquisition_state != ScanState::Complete {
        return false;
    }
    for (i, (folder, f, m)) in state
        .census
        .folders
        .iter()
        .flat_map(|(name, f)| {
            f.messages
                .values()
                .filter(|m| m.headers.direction == Direction::Received)
                .map(move |m| (name, f, m))
        })
        .take(7)
        .enumerate()
    {
        let evidence = match i {
            0 => ContentEvidence::Known {
                body: Body::Other,
                attachments: vec![Attachment {
                    name: "invoice.pdf".into(),
                    media_type: "application/pdf".into(),
                    bytes: b"%PDF-1.4\nSYNTHETIC-INVOICE-PRIVATE-BYTES\n%%EOF".to_vec(),
                }],
            },
            1 => ContentEvidence::Known {
                body: Body::Other,
                attachments: vec![Attachment {
                    name: "invoice.xml".into(),
                    media_type: "application/xml".into(),
                    bytes: b"<Invoice>SYNTHETIC-XML-PRIVATE</Invoice>".to_vec(),
                }],
            },
            2 => ContentEvidence::Known {
                body: Body::Receipt,
                attachments: vec![],
            },
            3 => ContentEvidence::Known {
                body: Body::LinkOnly,
                attachments: vec![],
            },
            4 => ContentEvidence::FilenameHint {
                names: vec!["ZUGFeRD-invoice.pdf".into()],
            },
            5 => ContentEvidence::Known {
                body: Body::Other,
                attachments: vec![Attachment {
                    name: "invoice.exe".into(),
                    media_type: "application/octet-stream".into(),
                    bytes: b"SYNTHETIC-UNSUPPORTED".to_vec(),
                }],
            },
            _ => ContentEvidence::Unknown,
        };
        let id = format!("{folder}/{}/{}", f.uidvalidity, m.uid);
        let body = match &evidence {
            ContentEvidence::Known {
                body: Body::LinkOnly,
                ..
            } => "https://invoice.example.test/document",
            _ => "Synthetic invoice receipt body only. No real mail.",
        };
        let mut original = format!(
            "From: {}\r\nTo: {}\r\nSubject: {}\r\nMessage-ID: <synthetic-account-{}@example.test>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=invoice-fixture\r\n\r\n--invoice-fixture\r\nContent-Type: text/plain\r\n\r\n{body}\r\n",
            m.headers
                .from
                .iter()
                .map(|a| a.address.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            m.headers
                .to
                .iter()
                .map(|a| a.address.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            m.headers.subject,
            m.uid
        );
        if let ContentEvidence::Known { attachments, .. } = &evidence {
            for a in attachments {
                original.push_str(&format!("--invoice-fixture\r\nContent-Type: {}\r\nContent-Disposition: attachment; filename=\"{}\"\r\n\r\n{}\r\n", a.media_type, a.name, String::from_utf8_lossy(&a.bytes)));
            }
        }
        original.push_str("--invoice-fixture--\r\n");
        state.invoices.sources.insert(
            id.clone(),
            Source {
                id,
                folder: folder.clone(),
                uidvalidity: f.uidvalidity,
                uid: m.uid,
                message_id_hash: m.headers.message_id_hash.clone(),
                original,
                evidence,
            },
        );
    }
    true
}

pub(crate) struct SyntheticTransport {
    store: EncryptedStore,
    state: AccountState,
}
impl SyntheticTransport {
    pub(crate) fn open(
        root: &Path,
        journal: &EncryptedStore,
        state: &AccountState,
    ) -> Result<Self, String> {
        let store = EncryptedStore::new(
            root.join("invoice-transport"),
            journal.keys().synthetic_invoice_keys(),
        )
        .map_err(|_| "Fake transport storage")?;
        if !store.path().exists() {
            store
                .save(&state.census)
                .map_err(|_| "Fake transport initialization")?;
        }
        let saved = store.load_state().map_err(|_| "Fake transport load")?;
        Ok(Self {
            store,
            state: saved,
        })
    }
    pub(crate) fn received(&self) -> &BTreeMap<String, Plan> {
        &self.state.invoices.synthetic_received
    }
    pub(crate) fn submit(&mut self, id: &str, plan: &Plan) -> Result<(), String> {
        if let Some(prior) = self.received().get(id) {
            return if prior == plan {
                Ok(())
            } else {
                Err("Fake acknowledgment differs".into())
            };
        }
        self.state
            .invoices
            .synthetic_received
            .insert(id.into(), plan.clone());
        self.store
            .save_state(&mut self.state)
            .map_err(|_| "Fake transport effect persistence".to_string())
    }
}
pub(crate) fn boundary(name: &str) {
    #[cfg(test)]
    if std::env::var("MAILROOM_INVOICE_BOUNDARY").as_deref() == Ok(name) {
        if let Some(root) = std::env::var_os("MAILROOM_DURABLE_BROWSER_FIXTURE") {
            std::fs::write(
                Path::new(&root).join(format!("invoice-reached-{name}")),
                b"synthetic boundary",
            )
            .unwrap();
            std::thread::sleep(std::time::Duration::from_secs(45));
        }
    }
    #[cfg(not(test))]
    let _ = name;
}
