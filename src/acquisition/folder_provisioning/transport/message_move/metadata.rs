use super::*;
use crate::backlog::imap_copy::ModSeq;

pub(super) fn sequence(text: &str) -> Result<ModSeq, Error> {
    if text.is_empty() || text.len() > 20 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Protocol);
    }
    ModSeq::new(text.parse().map_err(|_| Error::Protocol)?).map_err(|_| Error::Protocol)
}

pub(super) fn parse(row: &[Token]) -> Result<(Uid, Metadata, Option<ModSeq>), Error> {
    if row.first().and_then(Token::atom) != Some("*")
        || row
            .get(1)
            .and_then(Token::atom)
            .and_then(|n| n.parse::<u32>().ok())
            .is_none_or(|n| n == 0)
        || row.get(2).and_then(Token::atom) != Some("FETCH")
    {
        return Err(Error::Protocol);
    }
    if row.get(3) != Some(&Token::Open) || row.last() != Some(&Token::Close) {
        return Err(Error::Protocol);
    }
    let mut modseq = None;
    let mut uid = None;
    let mut flags = None;
    let mut date = None;
    let mut size = None;
    let mut hash = None;
    let mut at = 4;
    while at + 1 < row.len() {
        let key = row[at].atom().ok_or(Error::Protocol)?;
        at += 1;
        match key {
            "UID" => {
                if uid.is_some() {
                    return Err(Error::Protocol);
                }
                uid = Some(
                    Uid::new(
                        row.get(at)
                            .and_then(Token::atom)
                            .ok_or(Error::Protocol)?
                            .parse()
                            .map_err(|_| Error::Protocol)?,
                    )
                    .map_err(|_| Error::Protocol)?,
                );
                at += 1;
            }
            "MODSEQ" => {
                if modseq.is_some()
                    || row.get(at) != Some(&Token::Open)
                    || row.get(at + 2) != Some(&Token::Close)
                {
                    return Err(Error::Protocol);
                }
                modseq = Some(sequence(
                    row.get(at + 1)
                        .and_then(Token::atom)
                        .ok_or(Error::Protocol)?,
                )?);
                at += 3;
            }
            "RFC822.SIZE" => {
                if size.is_some() {
                    return Err(Error::Protocol);
                }
                size = Some(
                    row.get(at)
                        .and_then(Token::atom)
                        .ok_or(Error::Protocol)?
                        .parse()
                        .map_err(|_| Error::Protocol)?,
                );
                at += 1;
            }
            "INTERNALDATE" => {
                if date.is_some() {
                    return Err(Error::Protocol);
                }
                date = Some(row.get(at).ok_or(Error::Protocol)?.text()?);
                at += 1;
            }
            "FLAGS" => {
                if flags.is_some() || row.get(at) != Some(&Token::Open) {
                    return Err(Error::Protocol);
                }
                at += 1;
                let mut found = BTreeSet::new();
                while row.get(at) != Some(&Token::Close) {
                    let flag = row.get(at).and_then(Token::atom).ok_or(Error::Protocol)?;
                    if !flag.eq_ignore_ascii_case("\\Recent") && !found.insert(flag.into()) {
                        return Err(Error::Protocol);
                    }
                    if found.len() > 64 {
                        return Err(Error::Limit);
                    }
                    at += 1;
                }
                flags = Some(found);
                at += 1;
            }
            "BODY[HEADER.FIELDS" => {
                if hash.is_some()
                    || row.get(at) != Some(&Token::Open)
                    || row.get(at + 1).and_then(Token::atom) != Some("MESSAGE-ID")
                    || row.get(at + 2) != Some(&Token::Close)
                    || row.get(at + 3).and_then(Token::atom) != Some("]")
                {
                    return Err(Error::Protocol);
                }
                let Some(Token::String(raw)) = row.get(at + 4) else {
                    return Err(Error::Protocol);
                };
                hash = crate::census::parse_headers(raw, crate::census::Direction::Received)
                    .map_err(|_| Error::Protocol)?
                    .message_id_hash;
                at += 5;
            }
            _ => return Err(Error::Unsupported),
        }
    }
    if at != row.len() - 1 {
        return Err(Error::Protocol);
    }
    Ok((
        uid.ok_or(Error::Protocol)?,
        Metadata {
            message_id_hash: hash.ok_or(Error::Unsupported)?,
            size: size.ok_or(Error::Protocol)?,
            internal_date: date.ok_or(Error::Protocol)?,
            flags: flags.ok_or(Error::Protocol)?,
        },
        modseq,
    ))
}
