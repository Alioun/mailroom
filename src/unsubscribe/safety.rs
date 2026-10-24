use std::net::{IpAddr, Ipv4Addr};
use zeroize::Zeroizing;

pub(crate) struct Endpoint {
    pub host: String,
    uri: Zeroizing<String>,
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
        Ok(Self {
            host,
            uri: Zeroizing::new(raw.into()),
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
    pub subject: String,
    pub body: Zeroizing<String>,
}
impl Mail {
    pub(crate) fn parse(raw: &str, from: &str) -> Result<Self, String> {
        let rest = raw.strip_prefix("mailto:").ok_or("Invalid mailto")?;
        let (to, query) = rest.split_once('?').unwrap_or((rest, ""));
        if !crate::rules::config::mailbox(to) || !crate::rules::config::mailbox(from) {
            return Err("Mailto needs exactly one target and receiving identity".into());
        }
        let mut mail = Self {
            to: to.into(),
            from: from.into(),
            subject: String::new(),
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
                    *mail.body = value
                }
                _ => return Err("Unsupported or unsafe mailto field".into()),
            }
        }
        Ok(mail)
    }
    pub(crate) fn digest(&self) -> String {
        super::digest(
            &serde_json::to_vec(&(&self.to, &self.from, &self.subject, self.body.as_str()))
                .expect("mail encoding"),
        )
    }
}
fn decode(raw: &str) -> Result<String, String> {
    let mut out = Vec::new();
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
    String::from_utf8(out).map_err(|_| "Invalid mailto text".into())
}
