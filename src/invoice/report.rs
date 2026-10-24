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
fn evidence_reasons(e: &ContentEvidence) -> Vec<String> {
    match e {
        ContentEvidence::Unknown => vec!["Attachment structure and body content unknown".into()],
        ContentEvidence::FilenameHint { names } => vec![format!(
            "Filename hints only; no verified PDF/XML or ZUGFeRD content: {}",
            names.join(", ")
        )],
        ContentEvidence::Known { body, attachments } => {
            let mut reasons = vec![];
            if supported(attachments) {
                reasons.push("Known synthetic PDF/XML attachment bytes".into());
            }
            if !attachments.is_empty() && !supported(attachments) {
                reasons.push(
                    "Possible unsupported submission format; Qonto processing unknown".into(),
                );
            }
            if *body == Body::LinkOnly {
                reasons
                    .push("Known synthetic link-only content; no invoice document enclosed".into());
            }
            if *body == Body::Receipt {
                reasons.push("Receipt body may be convertible by Qonto; acceptance unknown".into());
            }
            reasons
        }
    }
}

pub(crate) fn report(
    state: &AccountState,
    provider: Provider,
    synthetic: bool,
    keys: &CensusKeys,
) -> Result<Report, String> {
    let mut report = Report { version: String::new(), semantics: SEMANTICS.into(), revision: state.revision, artifact: None, complete: state.census.acquisition_state == ScanState::Complete, count: 0, eligible_business: 0, unresolved: 0, rows: vec![], warnings: vec!["Prediction and prepared Sieve download do not establish handoff. Real handoff and Qonto acceptance remain unknown.".into(), "Target approval, sender restrictions, encryption, provider recipient provenance and MIME capabilities require independent evidence.".into(), "Keyword automatic copies remain omitted in full. A header census cannot establish attachment absence, link-only content or ZUGFeRD.".into()] };
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
                .map(|o| format!("{}: {}", o.rule, o.reason)),
        );
        let (_, config) = rules::config::documents(
            &w.input.documents.preset_yaml,
            &w.input.documents.override_yaml,
        )?;
        let proof = proof::prove(
            artifact,
            &state.census,
            w.input.as_of,
            &w.input.pins,
            Baseline::Current,
            None,
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
                    .filter(|s| synthetic && s.message_id_hash == m.headers.message_id_hash)
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
            let handoff = if state.invoices.sends.iter().any(|s| {
                s.plan.selection == d.id
                    && s.status == Status::Completed
                    && source == Some(&s.plan.source)
                    && config
                        .targets
                        .values()
                        .any(|t| t.address.eq_ignore_ascii_case(&s.plan.target))
            }) {
                "synthetic_handoff"
            } else if state.invoices.sends.iter().any(|s| {
                s.plan.selection == d.id
                    && source == Some(&s.plan.source)
                    && s.status != Status::Completed
            }) {
                "uncertain_synthetic_submission"
            } else {
                "real_handoff_unknown"
            };
            let mut reasons = evidence_reasons(&evidence);
            if keywords(&d.subject) {
                reasons.push("Invoice keyword in Subject".into());
            }
            if !business {
                reasons.push(
                    "Recipient role/evidence unresolved; retain Inbox without automatic copy"
                        .into(),
                );
            }
            if record.is_some_and(|m| {
                m.headers.evidence.contains_key("list-id")
                    || (m.headers.evidence.contains_key("list-unsubscribe")
                        && m.headers.evidence.contains_key("list-unsubscribe-post"))
            }) {
                reasons.push("Bulk evidence excludes keyword automatic copy".into());
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
            if handoff != "synthetic_handoff" || possible_unusable {
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
        report
            .warnings
            .push("Save a rule workspace to derive invoice roles and copy predictions".into());
    }
    report.count = report.rows.len();
    report.version = keys.binding_for(
        b"mailroom/invoice-report/v1",
        &serde_json::to_vec(&(&report, state)).map_err(|_| "Report encoding")?,
    );
    Ok(report)
}
