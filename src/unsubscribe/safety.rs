use std::net::{IpAddr, Ipv4Addr};
use zeroize::Zeroizing;

pub(crate) struct Endpoint {
    pub host: String,
    uri: Zeroizing<String>,
    target: Zeroizing<String>,
}
impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Endpoint([REDACTED])")
    }
}
impl Endpoint {
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        if raw.len() > 8192
            || !raw.is_ascii()
            || raw.bytes().any(|b| b <= 32 || b == 127)
            || raw.contains(['\\', '#'])
            || !raw.starts_with("https://")
        {
            return Err("Unsafe HTTPS unsubscribe endpoint".into());
        }
        let parsed: axum::http::Uri = raw.parse().map_err(|_| "Invalid HTTPS endpoint")?;
        let authority = parsed.authority().ok_or("Missing HTTPS authority")?;
        if parsed.scheme_str() != Some("https")
            || authority.as_str().contains(['@', '%'])
            || authority.port_u16().is_some_and(|p| p != 443)
        {
            return Err("Unsafe HTTPS authority".into());
        }
        let raw_host = authority.host().to_ascii_lowercase();
        let raw_authority = authority.as_str().to_ascii_lowercase();
        if raw_authority != raw_host && raw_authority != format!("{raw_host}:443") {
            return Err("Ambiguous HTTPS authority".into());
        }
        let bytes = raw.as_bytes();
        for i in 0..bytes.len() {
            if bytes[i] == b'%'
                && (i + 2 >= bytes.len()
                    || !bytes[i + 1].is_ascii_hexdigit()
                    || !bytes[i + 2].is_ascii_hexdigit())
            {
                return Err("Malformed URL encoding".into());
            }
        }
        let host = authority
            .host()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_ascii_lowercase();
        if host.parse::<IpAddr>().is_err()
            && (!crate::rules::config::domain(&host)
                || host.ends_with(".localhost")
                || host.ends_with(".local")
                || host
                    .split('.')
                    .all(|s| s.bytes().all(|b| b.is_ascii_digit()))
                || host.split('.').any(|s| s.starts_with("0x")))
        {
            return Err("Ambiguous or local host".into());
        }
        if host.parse::<IpAddr>().is_ok_and(|ip| !public(ip)) {
            return Err("Nonpublic destination".into());
        }
        let path = parsed.path_and_query().map_or("/", |p| p.as_str());
        let query_only = path.starts_with('?');
        let mut target =
            Zeroizing::new(String::with_capacity(path.len() + usize::from(query_only)));
        if query_only {
            target.push('/');
        }
        target.push_str(path);
        Ok(Self {
            host,
            uri: Zeroizing::new(raw.into()),
            target,
        })
    }
    pub(crate) fn bind(self, answers: &[IpAddr]) -> Result<BoundPost, String> {
        if answers.is_empty()
            || answers.iter().any(|ip| !public(*ip))
            || self
                .host
                .parse::<IpAddr>()
                .is_ok_and(|ip| !answers.iter().all(|a| *a == ip))
        {
            return Err("DNS answers contain an unchecked or nonpublic destination".into());
        }
        Ok(BoundPost {
            endpoint: self,
            addresses: answers.to_vec(),
        })
    }
}
pub(crate) struct BoundPost {
    endpoint: Endpoint,
    addresses: Vec<IpAddr>,
}
impl BoundPost {
    pub(crate) fn verify_peer(&self, peer: IpAddr) -> Result<(), String> {
        if self.addresses.contains(&peer) {
            Ok(())
        } else {
            Err("Peer changed after DNS validation".into())
        }
    }
    pub(crate) fn host(&self) -> &str {
        &self.endpoint.host
    }
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn addresses(&self) -> &[IpAddr] {
        &self.addresses
    }
    /// The origin-form request target: path and query, never logged.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn target(&self) -> &str {
        &self.endpoint.target
    }
    pub(crate) fn request_digest(&self) -> String {
        super::digest(self.endpoint.uri.as_bytes())
    }
    pub(crate) fn body(&self) -> &'static str {
        "List-Unsubscribe=One-Click"
    }
    pub(crate) fn headers(&self) -> [(&'static str, &'static str); 1] {
        [("Content-Type", "application/x-www-form-urlencoded")]
    }
    pub(crate) fn follow_redirect(&self, _location: &str) -> Result<(), String> {
        Err("Redirects are not followed".into())
    }
}
fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !matches!(a, 0 | 10 | 127 | 224..=255)
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 169 && b == 254)
        && !(a == 172 && (16..=31).contains(&b))
        && !(a == 192 && (b == 168 || b == 0 || (b == 88 && c == 99)))
        && !(a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
        && !(a == 203 && b == 0 && c == 113)
}
pub(crate) fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => {
            let s = ip.segments();
            // Only global unicast; mapped, translated and transition addresses are refused.
            (s[0] & 0xe000) == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x3fff && s[1] < 0x1000)
                && !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
        }
    }
}

pub(crate) struct Mail {
    pub to: String,
    pub from: String,
    pub subject: Zeroizing<String>,
    pub body: Zeroizing<String>,
}
impl Mail {
    pub(crate) fn parse(raw: &str, from: &str) -> Result<Self, String> {
        let rest = raw.strip_prefix("mailto:").ok_or("Invalid mailto")?;
        // A literal `#` would start a fragment for any other reader of this link.
        if rest.contains('#') {
            return Err("Invalid mailto".into());
        }
        let (to, query) = rest.split_once('?').unwrap_or((rest, ""));
        if !crate::rules::config::mailbox(to) || !crate::rules::config::mailbox(from) {
            return Err("Mailto needs exactly one target and receiving identity".into());
        }
        let mut mail = Self {
            to: to.into(),
            from: from.into(),
            subject: Zeroizing::new(String::new()),
            body: Zeroizing::new(String::new()),
        };
        let mut seen = std::collections::BTreeSet::new();
        for entry in query.split('&').filter(|s| !s.is_empty()) {
            let (key, value) = entry.split_once('=').ok_or("Invalid mailto query")?;
            if !seen.insert(key) {
                return Err("Duplicate mailto field".into());
            }
            let value = decode(value)?;
            match key {
                "subject" if !value.chars().any(char::is_control) => mail.subject = value,
                "body"
                    if !value
                        .chars()
                        .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t') =>
                {
                    mail.body = value
                }
                _ => return Err("Unsupported or unsafe mailto field".into()),
            }
        }
        Ok(mail)
    }
    pub(crate) fn digest(&self) -> String {
        // JSON escapes each input byte to at most six bytes.
        let capacity =
            (self.to.len() + self.from.len() + self.subject.len() + self.body.len()) * 6 + 13;
        let mut bytes = Zeroizing::new(Vec::with_capacity(capacity));
        let allocated = bytes.capacity();
        serde_json::to_writer(
            &mut *bytes,
            &(
                &self.to,
                &self.from,
                self.subject.as_str(),
                self.body.as_str(),
            ),
        )
        .expect("mail encoding");
        debug_assert_eq!(bytes.capacity(), allocated, "mail digest never reallocates");
        super::digest(&bytes)
    }
    /// The message SMTP submits: the previewed target, sender, subject and body
    /// with fixed framing derived from them and the review time `time`. Built in
    /// one zeroizing buffer sized up front, so the body leaves no unwiped copy.
    pub(crate) fn message(&self, time: i64) -> Option<Zeroizing<Vec<u8>>> {
        let date = chrono::DateTime::from_timestamp(time, 0)?.to_rfc2822();
        let subject = if self.subject.is_ascii() {
            self.subject.clone()
        } else {
            const PREFIX: &str = "=?UTF-8?B?";
            const SUFFIX: &str = "?=";
            let mut subject = Zeroizing::new(String::with_capacity(
                PREFIX.len() + data_encoding::BASE64.encode_len(self.subject.len()) + SUFFIX.len(),
            ));
            let allocated = subject.capacity();
            subject.push_str(PREFIX);
            data_encoding::BASE64.encode_append(self.subject.as_bytes(), &mut subject);
            subject.push_str(SUFFIX);
            debug_assert_eq!(subject.capacity(), allocated, "subject never reallocates");
            subject
        };
        let domain = self.from.rsplit_once('@')?.1;
        let id = &self.digest()[..32];
        let head = [
            "From: <",
            &self.from,
            ">\r\nTo: <",
            &self.to,
            ">\r\nSubject: ",
            &subject,
            "\r\nDate: ",
            &date,
            "\r\nMessage-ID: <",
            id,
            ".",
        ];
        let tail = [
            "@",
            domain,
            ">\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n",
        ];
        let stamp = time.to_string();
        // Every line end becomes CRLF, at most doubling the body, plus a final CRLF.
        let capacity = head.iter().chain(&tail).map(|p| p.len()).sum::<usize>()
            + stamp.len()
            + self.body.len() * 2
            + 2;
        let mut out = Zeroizing::new(Vec::with_capacity(capacity));
        let allocated = out.capacity();
        for part in head.iter().chain([&stamp.as_str()]).chain(&tail) {
            out.extend_from_slice(part.as_bytes());
        }
        let mut chars = self.body.bytes().peekable();
        let mut last = None;
        while let Some(b) = chars.next() {
            match b {
                b'\r' if chars.peek() == Some(&b'\n') => {}
                b'\r' | b'\n' => out.extend_from_slice(b"\r\n"),
                b => out.push(b),
            }
            last = Some(b);
        }
        if !matches!(last, Some(b'\r' | b'\n')) {
            out.extend_from_slice(b"\r\n");
        }
        debug_assert_eq!(out.capacity(), allocated, "the message never reallocates");
        Some(out)
    }
}
fn decode(raw: &str) -> Result<Zeroizing<String>, String> {
    // Decoding only shrinks, so the buffer never reallocates and leaves no copy.
    let mut out = Zeroizing::new(Vec::with_capacity(raw.len()));
    let mut bytes = raw.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let a = bytes
                .next()
                .and_then(|b| (b as char).to_digit(16))
                .ok_or("Invalid percent encoding")?;
            let c = bytes
                .next()
                .and_then(|b| (b as char).to_digit(16))
                .ok_or("Invalid percent encoding")?;
            out.push((a * 16 + c) as u8);
        } else {
            out.push(b);
        }
    }
    let text = std::str::from_utf8(&out).map_err(|_| "Invalid mailto text")?;
    let mut decoded = Zeroizing::new(String::with_capacity(out.len()));
    decoded.push_str(text);
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validated_request_targets_preserve_escapes_and_supply_an_origin_form_path() {
        for (uri, expected) in [
            ("https://shop.example", "/"),
            ("https://shop.example/", "/"),
            ("https://shop.example?token=fixture", "/?token=fixture"),
            ("https://shop.example?", "/?"),
            ("https://shop.example:443?token=fixture", "/?token=fixture"),
            (
                "https://shop.example/a%2fb/%C3%BC?token=a%2Bb&x=%23",
                "/a%2fb/%C3%BC?token=a%2Bb&x=%23",
            ),
        ] {
            let bound = Endpoint::parse(uri)
                .unwrap()
                .bind(&["93.184.216.34".parse().unwrap()])
                .unwrap();
            assert_eq!(bound.target(), expected);
            assert_eq!(
                bound.request_digest(),
                crate::unsubscribe::digest(uri.as_bytes())
            );
        }
        let host = format!("{}.{}.example", "a".repeat(63), "b".repeat(63));
        let uri = format!("https://{host}:443/leave?token={}", "x".repeat(4096));
        let bound = Endpoint::parse(&uri)
            .unwrap()
            .bind(&["93.184.216.34".parse().unwrap()])
            .unwrap();
        assert_eq!(bound.target(), format!("/leave?token={}", "x".repeat(4096)));
        for invalid in [
            "https://shop.example/path#fragment",
            "https://shop.example/%xy",
            "https://shop.example/%",
            "https://shop.example/a b",
            "https://user@shop.example/path",
            "https://shop.example:444/path",
        ] {
            assert!(Endpoint::parse(invalid).is_err());
        }
    }
}
