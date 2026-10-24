use super::{CensusError, Direction, DkimCoverage, HeaderRecord, Mailbox};
use mailparse::{MailAddr, MailHeaderMap};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const HEADER_FIELDS: &[&str] = &[
    "From",
    "Sender",
    "Reply-To",
    "To",
    "Cc",
    "Bcc",
    "Delivered-To",
    "X-Original-To",
    "Date",
    "Subject",
    "Message-ID",
    "In-Reply-To",
    "Return-Path",
    "List-Id",
    "List-Unsubscribe",
    "List-Unsubscribe-Post",
    "List-Post",
    "Auto-Submitted",
    "Precedence",
    "Feedback-ID",
    "Authentication-Results",
    "DKIM-Signature",
    "Received",
    "Received-SPF",
    "X-SimpleLogin-Type",
    "X-SimpleLogin-EmailLog-ID",
    "X-SimpleLogin-Envelope-From",
    "X-SimpleLogin-Envelope-To",
    "X-SimpleLogin-Original-From",
    "X-SimpleLogin-Original-List-Id",
    "X-SimpleLogin-Original-List-Unsubscribe",
    "X-AnonAddy-Original-Sender",
    "X-AnonAddy-Original-From-Header",
    "X-AnonAddy-Original-To",
    "X-AnonAddy-Original-Cc",
];

pub fn parse_headers(raw: &[u8], direction: Direction) -> Result<HeaderRecord, CensusError> {
    if raw.len() > 64 * 1024 || raw.contains(&0) {
        return Err(CensusError::InvalidHeaders);
    }
    let (headers, end) = mailparse::parse_headers(raw).map_err(|_| CensusError::InvalidHeaders)?;
    if end < raw.len() {
        return Err(CensusError::InvalidHeaders);
    }
    let mut evidence: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for header in &headers {
        let name = header.get_key();
        if name.eq_ignore_ascii_case("Message-ID") {
            continue;
        }
        if !HEADER_FIELDS
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(&name))
        {
            continue;
        }
        evidence
            .entry(name.to_ascii_lowercase())
            .or_default()
            .push(String::from_utf8_lossy(header.get_value_raw()).into_owned());
    }
    let addresses = |name: &str| -> Vec<Mailbox> {
        headers
            .get_all_values(name)
            .iter()
            .flat_map(|value| {
                mailparse::addrparse(value)
                    .map(|list| {
                        list.iter()
                            .flat_map(|entry| {
                                let single = |a: &mailparse::SingleInfo| Mailbox {
                                    address: a.addr.trim().to_lowercase(),
                                    display_name: a.display_name.clone(),
                                };
                                match entry {
                                    MailAddr::Single(a) => vec![single(a)],
                                    MailAddr::Group(group) => {
                                        group.addrs.iter().map(single).collect()
                                    }
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .collect()
    };
    let from = addresses("From");
    let relay = [
        "X-SimpleLogin-Original-From",
        "X-AnonAddy-Original-From-Header",
        "X-AnonAddy-Original-Sender",
    ]
    .iter()
    .find_map(|name| addresses(name).into_iter().next());
    let effective_sender = relay.or_else(|| from.first().cloned());
    let message_id_hash = headers
        .get_first_value("Message-ID")
        .map(|id| data_encoding::HEXLOWER.encode(&Sha256::digest(id.trim().as_bytes())));
    let return_path_domain = addresses("Return-Path")
        .first()
        .and_then(|a| a.address.rsplit_once('@').map(|(_, d)| d.to_owned()));
    Ok(HeaderRecord {
        direction,
        subject: headers.get_first_value("Subject").unwrap_or_default(),
        date: headers.get_first_value("Date"),
        message_id_hash,
        in_reply_to: headers.get_first_value("In-Reply-To").is_some(),
        from,
        sender: addresses("Sender"),
        reply_to: addresses("Reply-To"),
        to: addresses("To"),
        cc: addresses("Cc"),
        bcc: addresses("Bcc"),
        delivered_to: addresses("Delivered-To"),
        original_to: addresses("X-Original-To"),
        return_path_domain,
        effective_sender,
        registrable_domain: None,
        dkim_coverage: DkimCoverage::Unknown {
            reason: "No trusted provider verification or correlated passing signature".to_owned(),
        },
        evidence,
    })
}
