//! The real adapters' side of unsubscribe: the native one-click POST, SMTP
//! submission of local unsubscribe mail and the mail-app `mailto:` handoff.
//! The route reaches them only with a `policy::WritePermit`, which
//! `policy::authorize` still denies.
use super::{safety::Mail, *};

fn intent<'a>(
    state: &'a mut AccountState,
    id: &str,
    method: Method,
) -> Result<&'a mut Entry, String> {
    state
        .unsubscribe
        .history
        .iter_mut()
        .find(|e| e.id == id && e.status == Status::Intent && e.method == method)
        .ok_or_else(|| "No recorded intent for this action".into())
}

/// Posts a confirmed one-click intent once to `raw`, which must be the exact
/// previewed URI. Records the status and host only. The caller saved the
/// intent before and saves the result after.
pub(crate) fn post_one_click(
    state: &mut AccountState,
    id: &str,
    raw: &str,
    network: &dyn one_click::Network,
    tls: &native_tls::TlsConnector,
    timeout: std::time::Duration,
) -> Result<(), String> {
    let entry = intent(state, id, Method::OneClick)?;
    if digest(raw.as_bytes()) != entry.payload_digest {
        return Err("Exact payload changed".into());
    }
    let (host, outcome) = one_click::post(raw, network, tls, timeout);
    if host != entry.host {
        return Err("Exact payload changed".into());
    }
    match outcome {
        one_click::Outcome::Status { code } => {
            entry.http_status = Some(code);
            entry.status = Status::HttpObserved;
        }
        one_click::Outcome::NotSent { .. } => entry.status = Status::NotSent,
        one_click::Outcome::Uncertain { .. } => entry.status = Status::Uncertain,
    }
    Ok(())
}

/// Submits a confirmed local unsubscribe message once from the receiving
/// address, built from the exact previewed `mailto:` link.
pub(crate) fn submit_local_mail(
    state: &mut AccountState,
    id: &str,
    raw_mailto: &str,
    recipient: &str,
    server: &crate::send::smtp::Server,
    login: &crate::send::smtp::Login<'_>,
    timeouts: crate::send::smtp::Timeouts,
) -> Result<(), String> {
    use crate::send::smtp::{Outcome, submit};
    let entry = intent(state, id, Method::LocalMail)?;
    let mail = Mail::parse(raw_mailto, recipient)?;
    if mail.digest() != entry.payload_digest {
        return Err("Exact payload changed".into());
    }
    let message = mail.message(entry.time).ok_or("Message framing")?;
    let outcome = submit(server, login, &mail.from, &mail.to, &message, timeouts);
    entry.status = match outcome {
        Outcome::Accepted { .. } => Status::Submitted,
        Outcome::NotSent { .. } | Outcome::Rejected { .. } => Status::NotSent,
        Outcome::Uncertain { .. } => Status::Uncertain,
    };
    entry.smtp = Some(outcome);
    Ok(())
}

/// Opens a confirmed mail-app preparation's exact `mailto:` link in the mail app.
/// The entry records the handoff only; whether the person sent it stays unknown.
pub(crate) fn hand_off_mailto(
    state: &mut AccountState,
    id: &str,
    raw_mailto: &str,
    recipient: &str,
    launcher: &dyn crate::send::handoff::Launcher,
) -> Result<(), String> {
    let entry = state
        .unsubscribe
        .history
        .iter_mut()
        .find(|e| {
            e.id == id && e.status == Status::Prepared && e.method == Method::MailAppPreparation
        })
        .ok_or("No prepared mail-app action")?;
    let mail = Mail::parse(raw_mailto, recipient)?;
    let host = mail.to.rsplit_once('@').map(|(_, h)| h.to_owned());
    if mail.digest() != entry.payload_digest || host != entry.host {
        return Err("Exact payload changed".into());
    }
    let request = crate::send::handoff::Request::OpenMailto {
        uri: zeroize::Zeroizing::new(raw_mailto.into()),
        host: host.unwrap_or_default(),
    };
    if launcher.launch(&request).is_ok() {
        entry.status = Status::HandedOff;
    }
    Ok(())
}
