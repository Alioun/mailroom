use super::*;
use crate::{
    census::ScanState,
    rules::{
        Engine, EngineContext,
        config::{ForwardTrigger, Role},
        proof::{self, Baseline},
    },
};

fn keywords(subject: &str) -> bool {
    let s = subject.to_lowercase();
    ["invoice", "rechnung", "beleg", "quittung", "receipt"]
        .iter()
        .any(|w| s.contains(w))
}
fn supported(attachments: &[Attachment]) -> bool {
    attachments.iter().any(|a| {
        matches!(
            a.media_type.as_str(),
            "application/pdf" | "application/xml" | "text/xml"
        )
    })
}
fn evidence_reasons(e: &ContentEvidence) -> Vec<Diagnostic> {
    match e {
        ContentEvidence::Unknown => vec![Diagnostic::ContentUnknown],
        ContentEvidence::FilenameHint { names } => vec![Diagnostic::FilenameHint {
            names: names.clone(),
        }],
        ContentEvidence::Known { body, attachments } => {
            let mut reasons = vec![];
            if supported(attachments) {
                reasons.push(Diagnostic::DocumentContentTypes);
            }
            if !attachments.is_empty() && !supported(attachments) {
                reasons.push(Diagnostic::UnsupportedFormat);
            }
            if *body == Body::LinkOnly {
                reasons.push(Diagnostic::LinkOnly);
            }
            if *body == Body::Receipt {
                reasons.push(Diagnostic::ReceiptBody);
            }
            reasons
        }
    }
}

/// Part content types of every acquired original that still matches its census record.
fn mime_evidence(state: &AccountState, provider: Provider, synthetic: bool) -> proof::MimeEvidence {
    let mut evidence = proof::MimeEvidence::new();
    for (name, f) in &state.census.folders {
        for m in f.messages.values() {
            let Some(source) = state
                .invoices
                .sources
                .get(&format!("{name}/{}/{}", f.uidvalidity, m.uid))
                .filter(|s| {
                    (synthetic || s.acquired) && s.message_id_hash == m.headers.message_id_hash
                })
            else {
                continue;
            };
            let ContentEvidence::Known { attachments, .. } = &source.evidence else {
                continue;
            };
            let id = match (provider, &m.gmail_message_id) {
                (Provider::Gmail, Some(id)) => format!("gmail/{id}"),
                _ => source.id.clone(),
            };
            evidence.insert(
                id,
                std::iter::once("text/plain".to_owned())
                    .chain(
                        attachments
                            .iter()
                            .map(|a| a.media_type.to_ascii_lowercase()),
                    )
                    .collect(),
            );
        }
    }
    evidence
}

pub(crate) fn report(
    state: &AccountState,
    provider: Provider,
    synthetic: bool,
    keys: &CensusKeys,
) -> Result<Report, String> {
    let mut report = Report {
        version: String::new(),
        semantics: "invoice-report-evidence-v2".into(),
        revision: state.revision,
        artifact: None,
        complete: state.census.acquisition_state == ScanState::Complete,
        count: 0,
        eligible_business: 0,
        unresolved: 0,
        rows: vec![],
        warnings: vec![
            Diagnostic::HandoffUnknown,
            Diagnostic::IndependentEvidenceRequired,
            Diagnostic::KeywordMimeRequired,
        ],
    };
    if let Some(w) = &state.workspace {
        let mut context = EngineContext::local(
            state.census.account.as_str().into(),
            if provider == Provider::Gmail {
                Engine::GmailBacklog
            } else {
                Engine::MailboxSieve
            },
        );
        if synthetic {
            context.recipient_evidence = rules::EvidenceSource::SyntheticDeliveredTo;
            context.capabilities = [
                "fileinto",
                "copy",
                "duplicate",
                "relational",
                "comparator-i;ascii-numeric",
                "imap4flags",
                "mime",
            ]
            .into_iter()
            .map(String::from)
            .collect();
        }
        let artifact = crate::engine::artifact(&w.input.documents, &context)?;
        report.artifact = Some(artifact.identity().into());
        report.warnings.extend(
            artifact
                .omissions()
                .iter()
                .map(|o| Diagnostic::RuleOmitted {
                    rule: o.rule.clone(),
                    detail: o.reason.clone(),
                }),
        );
        let (_, config) = rules::config::documents(
            &w.input.documents.preset_yaml,
            &w.input.documents.override_yaml,
        )?;
        let proof = proof::prove_with_evidence(
            artifact,
            &state.census,
            w.input.as_of,
            &w.input.pins,
            Baseline::Current,
            None,
            &mime_evidence(state, provider, synthetic),
        )?;
        for d in proof.decisions {
            let location = state.census.folders.iter().find_map(|(name, f)| {
                f.messages
                    .values()
                    .find(|m| {
                        format!("{name}/{}/{}", f.uidvalidity, m.uid) == d.id
                            || (provider == Provider::Gmail
                                && m.gmail_message_id
                                    .as_ref()
                                    .is_some_and(|id| format!("gmail/{id}") == d.id))
                    })
                    .map(|m| (name, f, m))
            });
            let record = location.map(|(_, _, m)| m);
            let source = location.and_then(|(name, f, m)| {
                state
                    .invoices
                    .sources
                    .get(&format!("{name}/{}/{}", f.uidvalidity, m.uid))
                    .filter(|s| {
                        (synthetic || s.acquired) && s.message_id_hash == m.headers.message_id_hash
                    })
            });
            let evidence = source.map_or(ContentEvidence::Unknown, |s| s.evidence.clone());
            if !keywords(&d.subject) && source.is_none() {
                continue;
            }
            let business = synthetic
                && !d.review
                && record.is_some_and(|m| {
                    m.headers.delivered_to.len() == 1
                        && m.headers.delivered_to.iter().all(|a| {
                            config.roles.addresses.get(&a.address).or_else(|| {
                                a.address
                                    .rsplit_once('@')
                                    .and_then(|(_, domain)| config.roles.domains.get(domain))
                            }) == Some(&Role::Business)
                        })
                });
            let role = if business {
                "business"
            } else if d.review || !synthetic {
                "unresolved"
            } else {
                "personal_or_unfiltered"
            };
            if role == "personal_or_unfiltered" {
                continue;
            }
            let send = state.invoices.sends.iter().rev().find(|s| {
                s.plan.selection == d.id
                    && source == Some(&s.plan.source)
                    && s.status != Status::NotSent
            });
            let handoff = match send.map(|s| (&s.status, &s.plan.mode)) {
                Some((Status::Completed, _))
                    if send.unwrap().resolved == Some(crate::send::Resolution::Sent) =>
                {
                    "owner_recorded_sent"
                }
                Some((Status::Completed, SendMode::SyntheticTransport))
                    if config
                        .targets
                        .values()
                        .any(|t| t.address.eq_ignore_ascii_case(&send.unwrap().plan.target)) =>
                {
                    "synthetic_handoff"
                }
                Some((Status::Completed, SendMode::SyntheticTransport)) => "real_handoff_unknown",
                Some((Status::Completed, SendMode::Smtp)) => "smtp_accepted",
                Some((Status::HandedOff, _)) => "mail_app_handoff",
                Some((_, SendMode::SyntheticTransport)) => "uncertain_synthetic_submission",
                Some((Status::Intent | Status::Uncertain, _)) => "uncertain_submission",
                _ => "real_handoff_unknown",
            };
            let mut reasons = evidence_reasons(&evidence);
            if keywords(&d.subject) {
                reasons.push(Diagnostic::InvoiceSubject);
            }
            if !business {
                reasons.push(Diagnostic::RecipientHeld);
            }
            if record.is_some_and(|m| {
                m.headers.evidence.contains_key("list-id")
                    || (m.headers.evidence.contains_key("list-unsubscribe")
                        && m.headers.evidence.contains_key("list-unsubscribe-post"))
            }) {
                reasons.push(Diagnostic::BulkExcludesKeywordCopy);
            }
            let matching_rules = d
                .copy_eligibility
                .iter()
                .filter(|e| e.status == "eligible")
                .map(|e| e.rule.clone())
                .collect();
            let predicted_targets = d
                .copies
                .iter()
                .map(|c| {
                    config
                        .forwards
                        .iter()
                        .find(|f| f.id == c.rule)
                        .map_or(c.rule.clone(), |f| f.target.clone())
                })
                .collect();
            let proposal = business && keywords(&d.subject) && !config.forwards.iter().any(|f| matches!(&f.trigger, ForwardTrigger::ApprovedSenders { addresses } if addresses.contains(&d.from)));
            let possible_unusable = matches!(
                &evidence,
                ContentEvidence::Known {
                    body: Body::LinkOnly,
                    ..
                }
            ) || matches!(&evidence, ContentEvidence::Known { attachments, .. } if !attachments.is_empty() && !supported(attachments));
            if !matches!(
                handoff,
                "synthetic_handoff" | "smtp_accepted" | "owner_recorded_sent"
            ) || possible_unusable
            {
                if business {
                    report.eligible_business += 1;
                } else {
                    report.unresolved += 1;
                }
                report.rows.push(Row {
                    id: d.id,
                    subject: d.subject,
                    sender: d.from.clone(),
                    role: role.into(),
                    reasons,
                    evidence,
                    predicted_targets,
                    matching_rules,
                    copy_eligibility: d.copy_eligibility,
                    handoff: handoff.into(),
                    accounting_acceptance: "unknown".into(),
                    sender_proposal: proposal.then_some(d.from),
                    send_available: business
                        && source.is_some()
                        && handoff == "real_handoff_unknown",
                });
            }
        }
    } else {
        report.warnings.push(Diagnostic::WorkspaceRequired);
    }
    report.count = report.rows.len();
    report.version = keys.binding_for(
        b"mailroom/invoice-report/v1",
        &serde_json::to_vec(&(&report, state)).map_err(|_| "Report encoding")?,
    );
    Ok(report)
}

#[cfg(test)]
mod display_tests {
    use super::*;

    #[test]
    fn evidence_diagnostics_distinguish_observations_from_acceptance() {
        assert_eq!(
            evidence_reasons(&ContentEvidence::Unknown),
            vec![Diagnostic::ContentUnknown]
        );
        let names = vec!["<script>fixture</script>.pdf".into()];
        assert_eq!(
            evidence_reasons(&ContentEvidence::FilenameHint {
                names: names.clone()
            }),
            vec![Diagnostic::FilenameHint { names }]
        );
        for (media_type, expected) in [
            ("application/pdf", Diagnostic::DocumentContentTypes),
            ("application/xml", Diagnostic::DocumentContentTypes),
            ("application/octet-stream", Diagnostic::UnsupportedFormat),
        ] {
            let evidence = ContentEvidence::Known {
                body: Body::Receipt,
                attachments: vec![Attachment {
                    name: "fixture".into(),
                    media_type: media_type.into(),
                    bytes: vec![0],
                }],
            };
            assert_eq!(
                evidence_reasons(&evidence),
                vec![expected, Diagnostic::ReceiptBody]
            );
        }
        assert_eq!(
            evidence_reasons(&ContentEvidence::Known {
                body: Body::LinkOnly,
                attachments: vec![]
            }),
            vec![Diagnostic::LinkOnly]
        );
        println!(
            "MAILROOM_INVOICE_DIAGNOSTICS {}",
            serde_json::json!([
                Diagnostic::ContentUnknown,
                Diagnostic::FilenameHint {
                    names: vec!["<script>fixture</script>.pdf".into()]
                },
                Diagnostic::DocumentContentTypes,
                Diagnostic::UnsupportedFormat,
                Diagnostic::LinkOnly,
                Diagnostic::ReceiptBody,
                Diagnostic::InvoiceSubject,
                Diagnostic::RecipientHeld,
                Diagnostic::BulkExcludesKeywordCopy,
                Diagnostic::HandoffUnknown,
                Diagnostic::IndependentEvidenceRequired,
                Diagnostic::KeywordMimeRequired,
                Diagnostic::WorkspaceRequired,
                Diagnostic::RuleOmitted {
                    rule: "user.<script>fixture</script>".into(),
                    detail: "Required <script>fixture</script> capability unknown".into()
                },
            ])
        );
    }
}
