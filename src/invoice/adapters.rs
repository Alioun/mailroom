//! The real adapters' side of invoice send-on: acquired originals, SMTP
//! submission and mail-app handoff. Routes reach the send adapters only with a
//! `policy::WritePermit`, which `policy::authorize` still denies.
use super::*;

/// Most acquired originals one account keeps.
const ACQUIRED_LIMIT: usize = 64;

/// Keeps an original read from the provider for the census message
/// `folder/uidvalidity/uid`, so the report and proof use its MIME facts and a
/// send forwards its exact bytes. Returns the source ID.
pub(crate) fn record_original(
    state: &mut AccountState,
    folder: &str,
    uidvalidity: u32,
    uid: u32,
    original: crate::original::Original,
) -> Result<String, String> {
    let f = state
        .census
        .folders
        .get(folder)
        .ok_or("Source folder missing")?;
    let m = f.messages.get(&uid).ok_or("Source missing")?;
    if f.uidvalidity != uidvalidity
        || original.headers.message_id_hash.is_none()
        || original.headers.message_id_hash != m.headers.message_id_hash
    {
        return Err("Original does not match the census message".into());
    }
    let id = format!("{folder}/{uidvalidity}/{uid}");
    if !state.invoices.sources.contains_key(&id)
        && state
            .invoices
            .sources
            .values()
            .filter(|s| s.acquired)
            .count()
            >= ACQUIRED_LIMIT
    {
        return Err("Acquired original limit reached".into());
    }
    let evidence = original.evidence();
    let text = String::from_utf8(original.raw).map_err(|_| "Original is not UTF-8 text")?;
    state.invoices.sources.insert(
        id.clone(),
        Source {
            id: id.clone(),
            folder: folder.into(),
            uidvalidity,
            uid,
            message_id_hash: m.headers.message_id_hash.clone(),
            original: text,
            evidence,
            acquired: true,
        },
    );
    Ok(id)
}

fn intent<'a>(
    state: &'a mut AccountState,
    id: &str,
    mode: SendMode,
) -> Result<&'a mut Send, String> {
    state
        .invoices
        .sends
        .iter_mut()
        .find(|s| s.id == id && s.status == Status::Intent && s.plan.mode == mode)
        .ok_or_else(|| "No recorded intent for this send".into())
}

/// Submits a recorded SMTP intent exactly once and records the server's answer.
/// The caller saved the intent before and saves the result after. An
/// interrupted or unanswered submission stays uncertain and is never retried.
pub(crate) fn submit_smtp(
    state: &mut AccountState,
    id: &str,
    server: &crate::send::smtp::Server,
    login: &crate::send::smtp::Login<'_>,
    timeouts: crate::send::smtp::Timeouts,
) -> Result<(), String> {
    use crate::send::smtp::{Outcome, submit};
    let send = intent(state, id, SendMode::Smtp)?;
    let outcome = submit(
        server,
        login,
        &send.plan.sending_address,
        &send.plan.target,
        send.plan.source.original.as_bytes(),
        timeouts,
    );
    send.status = match outcome {
        Outcome::Accepted { .. } => Status::Completed,
        Outcome::NotSent { .. } | Outcome::Rejected { .. } => Status::NotSent,
        Outcome::Uncertain { .. } => Status::Uncertain,
    };
    send.smtp = Some(outcome);
    Ok(())
}

/// Asks the mail app to open a recorded handoff. Never records the message as sent.
pub(crate) fn hand_off(
    state: &mut AccountState,
    id: &str,
    launcher: &dyn crate::send::handoff::Launcher,
) -> Result<(), String> {
    let send = intent(state, id, SendMode::MailAppHandoff)?;
    let request = handoff_request(&send.plan).ok_or("Original has no Message-ID")?;
    send.status = if launcher.launch(&request).is_ok() {
        Status::HandedOff
    } else {
        Status::NotSent
    };
    Ok(())
}
