use super::*;
use crate::census::{Census, EncryptedStore, HeaderRecord};
use std::path::Path;
const SIGNATURE: &str = "v=1; d=shop.example; s=fixture; h=list-unsubscribe:list-unsubscribe-post; b=synthetic-signature";
#[derive(Clone)]
pub(crate) struct ProviderObservation {
    pub signature_digest: String,
    pub unsubscribe_digest: String,
    pub pass: bool,
}
fn authenticate(h: &HeaderRecord, observation: Option<ProviderObservation>) -> String {
    let Some(observation) = observation else {
        return "unknown_provider_provenance".into();
    };
    let headers_digest = digest(
        &serde_json::to_vec(&(
            h.evidence.get("list-unsubscribe"),
            h.evidence.get("list-unsubscribe-post"),
        ))
        .expect("Header encoding"),
    );
    if observation.unsubscribe_digest != headers_digest {
        return "unknown_changed_authenticated_headers".into();
    }
    if !observation.pass {
        return "synthetic_provider_fail".into();
    }
    let Some(signatures) = h.evidence.get("dkim-signature") else {
        return "unknown_no_correlated_signature".into();
    };
    let Some(signature) = signatures
        .iter()
        .find(|s| digest(s.as_bytes()) == observation.signature_digest)
    else {
        return "unknown_wrong_signature".into();
    };
    let names: Vec<_> = signature
        .split(';')
        .filter_map(|s| s.trim().strip_prefix("h="))
        .flat_map(|s| s.split(':'))
        .map(|s| s.trim().to_ascii_lowercase())
        .collect();
    if names.contains(&"list-unsubscribe".into())
        && names.contains(&"list-unsubscribe-post".into())
        && h.evidence
            .get("list-unsubscribe")
            .is_some_and(|v| v.len() == 1)
        && h.evidence
            .get("list-unsubscribe-post")
            .is_some_and(|v| v.len() == 1)
    {
        "synthetic_provider_pass_both_same_signature".into()
    } else {
        "unknown_incomplete_header_coverage".into()
    }
}
pub(crate) struct InstalledSynthetic {
    store: EncryptedStore,
    saved: AccountState,
}
impl InstalledSynthetic {
    pub(crate) fn open(
        root: &Path,
        journal: &EncryptedStore,
        state: &AccountState,
    ) -> Result<Self, String> {
        let store = EncryptedStore::new(
            root.join("unsubscribe-transport"),
            journal.keys().synthetic_unsubscribe_keys(),
        )
        .map_err(|_| "Fake unsubscribe storage")?;
        if !store.path().exists() {
            store
                .save(&Census::new(state.census.account.clone()))
                .map_err(|_| "Fake unsubscribe initialization")?;
        }
        let saved = store.load_state().map_err(|_| "Fake unsubscribe load")?;
        Ok(Self { store, saved })
    }
    pub(crate) fn authentication(&self, h: &HeaderRecord) -> String {
        let signature_present = h
            .evidence
            .get("dkim-signature")
            .is_some_and(|v| v.iter().any(|s| s == SIGNATURE));
        let expected_unsubscribe = vec![
            "<https://shop.example/remove?token=synthetic-private-token>, <mailto:leave@shop.example?subject=unsubscribe&body=synthetic-mail-token>",
        ];
        let expected_post = vec!["List-Unsubscribe=One-Click"];
        let observation = signature_present.then(|| ProviderObservation {
            signature_digest: digest(SIGNATURE.as_bytes()),
            unsubscribe_digest: digest(
                &serde_json::to_vec(&(Some(expected_unsubscribe), Some(expected_post)))
                    .expect("Fixture headers"),
            ),
            pass: true,
        });
        authenticate(h, observation)
    }
    pub(crate) fn original_survives(&self, h: &HeaderRecord) -> bool {
        !h.evidence
            .keys()
            .any(|k| k.starts_with("x-simplelogin-") || k.starts_with("x-anonaddy-"))
    }
    pub(crate) fn post(&self, raw: &str) -> Result<safety::BoundPost, String> {
        let endpoint = safety::Endpoint::parse(raw)?;
        // The installed fake resolves only this fixture host. It never resolves or contacts a network.
        if endpoint.host != "shop.example" {
            return Err("No synthetic destination fixture installed".into());
        }
        endpoint.bind(&["93.184.216.34".parse().unwrap()])
    }
    #[cfg(test)]
    pub(super) fn saved_for_test(&self) -> &AccountState {
        &self.saved
    }
    pub(crate) fn received(&self) -> &BTreeMap<String, Acknowledgment> {
        &self.saved.unsubscribe.synthetic_acknowledgments
    }
    pub(crate) fn submit(
        &mut self,
        id: &str,
        entry: &Entry,
        source: &crate::census::MessageRecord,
        recipient: &str,
    ) -> Result<(), String> {
        let uris = candidates::uris(&source.headers);
        let ack = match entry.method {
            Method::OneClick => {
                if self.authentication(&source.headers)
                    != "synthetic_provider_pass_both_same_signature"
                {
                    return Err("Authentication changed".into());
                }
                let raw = uris
                    .iter()
                    .find(|s| s.starts_with("https://"))
                    .ok_or("Missing exact HTTPS URI")?;
                let post = self.post(raw)?;
                post.verify_peer("93.184.216.34".parse().unwrap())?;
                if post.follow_redirect("https://unchecked.example/").is_ok() {
                    return Err("Redirect policy failed".into());
                }
                Acknowledgment {
                    payload_digest: post.request_digest(),
                    host: Some(post.host().into()),
                    http_status: Some(202),
                    mail_handoff: false,
                    verb: "POST".into(),
                    form: Some(post.body().into()),
                    headers: post
                        .headers()
                        .into_iter()
                        .map(|(k, v)| (k.into(), v.into()))
                        .collect(),
                }
            }
            Method::LocalMail => {
                let mail = safety::Mail::parse(
                    uris.iter()
                        .find(|s| s.starts_with("mailto:"))
                        .ok_or("Missing mailto")?,
                    recipient,
                )?;
                Acknowledgment {
                    payload_digest: mail.digest(),
                    host: mail.to.split_once('@').map(|(_, h)| h.into()),
                    http_status: None,
                    mail_handoff: true,
                    verb: "MAIL".into(),
                    form: None,
                    headers: BTreeMap::new(),
                }
            }
            _ => return Err("No external effect for preparation/mute/keep".into()),
        };
        if ack.payload_digest != entry.payload_digest || ack.host != entry.host {
            return Err("Exact payload changed".into());
        }
        if let Some(prior) = self.received().get(id) {
            return if prior == &ack {
                Ok(())
            } else {
                Err("Fake acknowledgment conflict".into())
            };
        }
        self.saved
            .unsubscribe
            .synthetic_acknowledgments
            .insert(id.into(), ack);
        self.store
            .save_state(&mut self.saved)
            .map_err(|_| "Fake unsubscribe effect persistence".into())
    }
}
pub(crate) fn boundary(name: &str) {
    #[cfg(test)]
    if std::env::var("MAILROOM_UNSUBSCRIBE_BOUNDARY").as_deref() == Ok(name) {
        if let Some(root) = std::env::var_os("MAILROOM_DURABLE_BROWSER_FIXTURE") {
            std::fs::write(
                Path::new(&root).join(format!("unsubscribe-reached-{name}")),
                b"synthetic boundary",
            )
            .unwrap();
            std::thread::sleep(std::time::Duration::from_secs(45));
        }
    }
    #[cfg(not(test))]
    let _ = name;
}
#[cfg(test)]
pub(super) fn auth_test(h: &HeaderRecord, signature: &str, pass: bool) -> String {
    authenticate(
        h,
        Some(ProviderObservation {
            signature_digest: digest(signature.as_bytes()),
            unsubscribe_digest: digest(
                &serde_json::to_vec(&(
                    h.evidence.get("list-unsubscribe"),
                    h.evidence.get("list-unsubscribe-post"),
                ))
                .unwrap(),
            ),
            pass,
        }),
    )
}
