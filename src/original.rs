//! Typed facts read from the exact bytes of one original message.
use crate::{
    census::{Direction, HeaderRecord, parse_headers},
    invoice::{Attachment, Body, ContentEvidence},
};
use mailparse::{DispositionType, ParsedMail};

/// Largest original Mailroom reads or forwards.
pub(crate) const LIMIT: usize = 25 * 1024 * 1024;
const PARTS: usize = 256;
const DEPTH: usize = 8;

/// One original message: its exact bytes and the facts parsed from them.
pub(crate) struct Original {
    pub raw: Vec<u8>,
    pub headers: HeaderRecord,
    pub attachments: Vec<Attachment>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParseError {
    TooLarge,
    Malformed,
    TooComplex,
}

impl Original {
    pub(crate) fn parse(raw: Vec<u8>) -> Result<Self, ParseError> {
        if raw.len() > LIMIT {
            return Err(ParseError::TooLarge);
        }
        let (_, end) = mailparse::parse_headers(&raw).map_err(|_| ParseError::Malformed)?;
        let headers =
            parse_headers(&raw[..end], Direction::Received).map_err(|_| ParseError::Malformed)?;
        let mail = mailparse::parse_mail(&raw).map_err(|_| ParseError::Malformed)?;
        let mut attachments = vec![];
        let mut parts = 0;
        collect(&mail, 0, &mut parts, &mut attachments)?;
        Ok(Self {
            raw,
            headers,
            attachments,
        })
    }
    /// Evidence for the invoice report. The body is never classified from real mail.
    pub(crate) fn evidence(&self) -> ContentEvidence {
        ContentEvidence::Known {
            body: Body::Other,
            attachments: self.attachments.clone(),
        }
    }
}

fn collect(
    part: &ParsedMail<'_>,
    depth: usize,
    parts: &mut usize,
    into: &mut Vec<Attachment>,
) -> Result<(), ParseError> {
    *parts += 1;
    if depth > DEPTH || *parts > PARTS {
        return Err(ParseError::TooComplex);
    }
    if !part.subparts.is_empty() {
        for sub in &part.subparts {
            collect(sub, depth + 1, parts, into)?;
        }
        return Ok(());
    }
    let disposition = part.get_content_disposition();
    let name = disposition
        .params
        .get("filename")
        .or_else(|| part.ctype.params.get("name"))
        .map(|n| file_name(n));
    let media_type = part.ctype.mimetype.to_ascii_lowercase();
    // Every part but an unnamed inline text body is a document, since the Sieve
    // MIME test matches inline parts too.
    let attached = disposition.disposition == DispositionType::Attachment
        || name.is_some()
        || !matches!(media_type.as_str(), "text/plain" | "text/html");
    if attached {
        into.push(Attachment {
            name: name.unwrap_or_default(),
            media_type,
            bytes: part.get_body_raw().map_err(|_| ParseError::Malformed)?,
        });
    }
    Ok(())
}

/// The last path segment, without control characters, at most 255 characters.
fn file_name(raw: &str) -> String {
    raw.rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(255)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(parts: &str) -> Vec<u8> {
        format!("From: Billing <billing@shop.example>\r\nTo: books@work.example.test\r\nSubject: Rechnung 42\r\nMessage-ID: <fixture-1@shop.example>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nSee attached.\r\n{parts}--b--\r\n").into_bytes()
    }

    #[test]
    fn typed_facts_name_pdf_and_xml_attachments_and_keep_census_headers() {
        let raw = message(
            "--b\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"../dir/Rechnung.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n--b\r\nContent-Type: text/xml; name=\"factur-x.xml\"\r\n\r\n<Invoice/>\r\n",
        );
        let original = Original::parse(raw.clone()).unwrap();
        assert_eq!(original.raw, raw, "exact bytes are kept");
        assert_eq!(original.headers.from[0].address, "billing@shop.example");
        assert!(original.headers.message_id_hash.is_some());
        assert_eq!(original.headers.subject, "Rechnung 42");
        let facts: Vec<_> = original
            .attachments
            .iter()
            .map(|a| (a.name.as_str(), a.media_type.as_str(), a.bytes.clone()))
            .collect();
        assert_eq!(
            facts,
            [
                ("Rechnung.pdf", "application/pdf", b"%PDF-1.4\n".to_vec()),
                ("factur-x.xml", "text/xml", b"<Invoice/>".to_vec()),
            ]
        );
    }

    #[test]
    fn body_only_mail_has_no_attachments_and_bounds_refuse_oversized_or_deep_mail() {
        let original = Original::parse(message("")).unwrap();
        assert!(original.attachments.is_empty());
        assert!(matches!(
            original.evidence(),
            ContentEvidence::Known { body: Body::Other, ref attachments } if attachments.is_empty()
        ));
        assert_eq!(
            Original::parse(vec![b'x'; LIMIT + 1]).err(),
            Some(ParseError::TooLarge)
        );
        let mut nested = String::from("Content-Type: text/plain\r\n\r\nleaf\r\n");
        for depth in 0..12 {
            nested = format!(
                "Content-Type: multipart/mixed; boundary=d{depth}\r\n\r\n--d{depth}\r\n{nested}--d{depth}--\r\n"
            );
        }
        let deep = format!("From: a@example.test\r\n{nested}");
        assert_eq!(
            Original::parse(deep.into_bytes()).err(),
            Some(ParseError::TooComplex)
        );
    }
}
