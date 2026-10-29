use super::*;
use crate::backlog::imap_copy::ModSeq;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Persistence {
    Unknown,
    Unavailable,
    Persistent(ModSeq),
}
pub(super) struct Selection {
    expected: UidValidity,
    validity: Option<UidValidity>,
    writable: bool,
    persistence: Persistence,
}
impl Selection {
    pub(super) fn new(mailbox: &MailboxIdentity) -> Self {
        Self {
            expected: mailbox.uidvalidity,
            validity: None,
            writable: false,
            persistence: Persistence::Unknown,
        }
    }
    pub(super) fn observe(&mut self, row: &[Token], tagged: bool) -> Result<(), Error> {
        if row.len() < 2 {
            return Err(Error::Protocol);
        }
        if let Some(status) = row.get(1).and_then(Token::atom) {
            match status {
                "NO" => return Err(Error::No),
                "BAD" => return Err(Error::Bad),
                "BYE" => return Err(Error::Bye),
                _ => {}
            }
        }
        if let Some(code) = row
            .get(2)
            .and_then(Token::atom)
            .filter(|v| v.starts_with('['))
        {
            match code {
                "[UIDNOTSTICKY]" | "[READ-ONLY]" => return Err(Error::Unsupported),
                "[READ-WRITE]" => {
                    if !tagged {
                        return Err(Error::Protocol);
                    }
                    if self.writable {
                        return Err(Error::Protocol);
                    }
                    self.writable = true;
                }
                "[NOMODSEQ]" => {
                    if self.persistence != Persistence::Unknown {
                        return Err(Error::Protocol);
                    }
                    self.persistence = Persistence::Unavailable;
                }
                _ if code.starts_with("[UIDVALIDITY ") => {
                    let text = code
                        .strip_prefix("[UIDVALIDITY ")
                        .and_then(|v| v.strip_suffix(']'))
                        .ok_or(Error::Protocol)?;
                    if text.is_empty()
                        || text.len() > 10
                        || !text.bytes().all(|b| b.is_ascii_digit())
                    {
                        return Err(Error::Protocol);
                    }
                    let validity = UidValidity::new(text.parse().map_err(|_| Error::Protocol)?)
                        .map_err(|_| Error::Protocol)?;
                    if self.validity.replace(validity).is_some() {
                        return Err(Error::Protocol);
                    }
                }
                _ if code.starts_with("[HIGHESTMODSEQ ") => {
                    let text = code
                        .strip_prefix("[HIGHESTMODSEQ ")
                        .and_then(|v| v.strip_suffix(']'))
                        .ok_or(Error::Protocol)?;
                    let value = metadata::sequence(text)?;
                    if self.persistence != Persistence::Unknown {
                        return Err(Error::Protocol);
                    }
                    self.persistence = Persistence::Persistent(value);
                }
                _ if (code.starts_with("[PERMANENTFLAGS ")
                    || code.starts_with("[UIDNEXT ")
                    || code.starts_with("[UNSEEN "))
                    && code.ends_with(']') => {}
                _ => return Err(Error::UnknownCode),
            }
        }
        if tagged {
            if row.get(1).and_then(Token::atom) != Some("OK") {
                return Err(Error::Protocol);
            }
            if self.validity != Some(self.expected) || !self.writable {
                return Err(Error::Unstable);
            }
        }
        Ok(())
    }
}
