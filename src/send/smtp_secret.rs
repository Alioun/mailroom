//! The only code that handles an account password for SMTP submission.
//!
//! Secret flow: the password is read once through the approved
//! `CredentialBackend::get`, held in a zeroizing `SecretValue`, encoded into one
//! zeroizing AUTH PLAIN line and written to the TLS session. Every buffer this
//! code or `Session::reply` owns is wiped when it drops, on success and on every
//! failure; the TLS library's and `BufReader`'s own buffers are not. Nothing
//! here formats, logs, stores or returns the password or the server's reply
//! text; callers learn only whether authentication succeeded and the reply code.
use super::smtp::{Login, Session};
use zeroize::Zeroizing;

/// Authenticates with AUTH PLAIN (RFC 4616), which `submit` only reaches over TLS.
/// Returns the server's reply code when authentication did not succeed.
pub(super) fn authenticate(session: &mut Session, login: &Login<'_>) -> Result<(), Option<u16>> {
    if !session.extensions.contains("AUTH=PLAIN")
        || login.user.is_empty()
        || login.user.contains(['\0', '\r', '\n'])
    {
        return Err(None);
    }
    let password = login.backend.get(login.reference).map_err(|_| None)?;
    if password.bytes().is_empty() || password.bytes().contains(&0) {
        return Err(None);
    }
    let mut credentials = Zeroizing::new(Vec::with_capacity(
        login.user.len() + password.bytes().len() + 2,
    ));
    credentials.push(0);
    credentials.extend_from_slice(login.user.as_bytes());
    credentials.push(0);
    credentials.extend_from_slice(password.bytes());
    drop(password);
    // Room for the whole line, so encoding never reallocates and leaves an unwiped copy.
    let mut line = Zeroizing::new(String::with_capacity(credentials.len() * 2 + 32));
    line.push_str("AUTH PLAIN ");
    data_encoding::BASE64.encode_append(&credentials, &mut line);
    line.push_str("\r\n");
    drop(credentials);
    session.write_line(line.as_bytes()).map_err(|_| None)?;
    drop(line);
    // A server may echo what it received; the reply's lines wipe themselves on
    // drop, here and on every error path inside `reply`.
    let code = session.reply().map(|reply| reply.code);
    match code {
        Ok(235) => Ok(()),
        Ok(code) => Err(Some(code)),
        Err(_) => Err(None),
    }
}
