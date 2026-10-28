//! One-click authority from the census's recorded provider evidence (RFC 8058
//! section 4, RFC 8601).
//!
//! A one-click POST is offered only when the topmost Authentication-Results
//! header carries the provider's authserv-id, no other header claims that id,
//! a `dkim=pass` result there correlates with the message's DKIM-Signature
//! headers, and every signature it can name covers both List-Unsubscribe and
//! List-Unsubscribe-Post, each present exactly once. Anything missing or
//! unknown offers no one-click POST.
use crate::census::HeaderRecord;

/// The only evidence that authorizes a one-click POST.
pub(crate) const PASS: &str = "provider_dkim_pass_both";

/// Removes RFC 5322 comments and folding, so tokens split cleanly.
fn uncomment(value: &str) -> String {
    let mut depth = 0usize;
    let mut out = String::with_capacity(value.len());
    let mut quoted = false;
    let mut escaped = false;
    for c in value.chars() {
        match c {
            _ if escaped => {
                escaped = false;
                if depth == 0 {
                    out.push(c);
                }
            }
            '\\' => escaped = true,
            '"' if depth == 0 => {
                quoted = !quoted;
                out.push(c);
            }
            '(' if !quoted => depth += 1,
            ')' if !quoted && depth > 0 => depth -= 1,
            '\r' | '\n' | '\t' if depth == 0 => out.push(' '),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

/// The authserv-id that opens one Authentication-Results value.
fn authserv(value: &str) -> Option<String> {
    let clean = uncomment(value);
    let first = clean.split(';').next()?.split_whitespace().next()?;
    Some(first.to_ascii_lowercase())
}

/// The properties of each `dkim=pass` result in one Authentication-Results value.
fn passing(value: &str) -> Vec<Vec<(String, String)>> {
    uncomment(value)
        .split(';')
        .skip(1)
        .filter_map(|resinfo| {
            let mut tokens = resinfo.split_whitespace();
            let (method, result) = tokens.next()?.split_once('=')?;
            if !method.eq_ignore_ascii_case("dkim") || !result.eq_ignore_ascii_case("pass") {
                return None;
            }
            Some(
                tokens
                    .filter_map(|t| t.split_once('='))
                    .map(|(k, v)| (k.to_ascii_lowercase(), unquote(v).to_owned()))
                    .collect(),
            )
        })
        .collect()
}

/// One DKIM-Signature's tags, with folding whitespace removed from values.
fn tags(signature: &str) -> Vec<(String, String)> {
    signature
        .split(';')
        .filter_map(|t| t.split_once('='))
        .map(|(k, v)| {
            (
                k.trim().to_ascii_lowercase(),
                v.chars().filter(|c| !c.is_whitespace()).collect(),
            )
        })
        .collect()
}

fn tag<'a>(tags: &'a [(String, String)], name: &str) -> Option<&'a str> {
    tags.iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// Whether a passing result names this signature: same `d=`, and the same `s=`
/// and a matching `b=` prefix where the result records them.
fn names(result: &[(String, String)], signature: &[(String, String)]) -> bool {
    let property = |name: &str| result.iter().find(|(k, _)| k == name).map(|(_, v)| v);
    let Some(domain) = property("header.d") else {
        return false;
    };
    tag(signature, "d").is_some_and(|d| d.eq_ignore_ascii_case(domain))
        && property("header.s").is_none_or(|s| tag(signature, "s") == Some(s.as_str()))
        && property("header.b").is_none_or(|b| {
            b.len() >= 8 && tag(signature, "b").is_some_and(|sig| sig.starts_with(b.as_str()))
        })
}

fn covers_both(signature: &[(String, String)]) -> bool {
    let signed: Vec<String> = tag(signature, "h")
        .unwrap_or_default()
        .split(':')
        .map(str::to_ascii_lowercase)
        .collect();
    signed.iter().any(|h| h == "list-unsubscribe")
        && signed.iter().any(|h| h == "list-unsubscribe-post")
}

/// The one-click evidence of `h` under the provider's `trusted` authserv-id.
pub(crate) fn one_click(h: &HeaderRecord, trusted: Option<&str>) -> &'static str {
    let Some(trusted) = trusted.map(str::to_ascii_lowercase) else {
        return "unknown_provider_authserv_id";
    };
    let results = h
        .evidence
        .get("authentication-results")
        .map(Vec::as_slice)
        .unwrap_or_default();
    let Some(top) = results.first() else {
        return "unknown_no_provider_results";
    };
    if authserv(top).as_deref() != Some(&trusted) {
        return "unknown_no_provider_results";
    }
    if results[1..]
        .iter()
        .any(|r| authserv(r).as_deref() == Some(&trusted))
    {
        return "unknown_ambiguous_provider_results";
    }
    let once = |name: &str| h.evidence.get(name).is_some_and(|v| v.len() == 1);
    if !once("list-unsubscribe") || !once("list-unsubscribe-post") {
        return "unknown_repeated_or_missing_unsubscribe_headers";
    }
    let passes = passing(top);
    if passes.is_empty() {
        return "provider_dkim_no_pass";
    }
    let signatures: Vec<_> = h
        .evidence
        .get("dkim-signature")
        .map(|v| v.iter().map(|s| tags(s)).collect())
        .unwrap_or_default();
    let mut correlated = false;
    for result in &passes {
        let named: Vec<_> = signatures.iter().filter(|s| names(result, s)).collect();
        correlated |= !named.is_empty();
        if !named.is_empty() && named.iter().all(|s| covers_both(s)) {
            return PASS;
        }
    }
    if correlated {
        "unknown_incomplete_header_coverage"
    } else {
        "unknown_no_correlated_signature"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGNATURE: &str = "v=1; a=rsa-sha256; d=shop.example; s=s1;\r\n\th=from:list-unsubscribe:list-unsubscribe-post; b=AbCdEf12Gh\r\n\tIj34";
    fn headers(results: &[&str], signatures: &[&str]) -> HeaderRecord {
        let mut raw = String::new();
        for r in results {
            raw.push_str(&format!("Authentication-Results: {r}\r\n"));
        }
        for s in signatures {
            raw.push_str(&format!("DKIM-Signature: {s}\r\n"));
        }
        raw.push_str("From: news@shop.example\r\nList-Unsubscribe: <https://shop.example/u>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n\r\n");
        crate::census::parse_headers(raw.as_bytes(), crate::census::Direction::Received).unwrap()
    }

    #[test]
    fn each_evidence_case_decides_the_one_click_offer() {
        let pass = "mx.provider.test; dkim=pass (2048-bit key) header.d=shop.example header.s=s1 header.b=\"AbCdEf12\"; spf=pass";
        let cases: &[(&[&str], &[&str], Option<&str>, &str)] = &[
            (&[pass], &[SIGNATURE], Some("MX.provider.test"), PASS),
            (&[pass], &[SIGNATURE], None, "unknown_provider_authserv_id"),
            (
                &[],
                &[SIGNATURE],
                Some("mx.provider.test"),
                "unknown_no_provider_results",
            ),
            (
                &["forged.example; dkim=pass header.d=shop.example", pass],
                &[SIGNATURE],
                Some("mx.provider.test"),
                "unknown_no_provider_results",
            ),
            (
                &[pass, "mx.provider.test; dkim=pass header.d=shop.example"],
                &[SIGNATURE],
                Some("mx.provider.test"),
                "unknown_ambiguous_provider_results",
            ),
            (
                &["mx.provider.test; dkim=fail header.d=shop.example; dkim=neutral"],
                &[SIGNATURE],
                Some("mx.provider.test"),
                "provider_dkim_no_pass",
            ),
            (
                &["mx.provider.test; spf=pass (dkim=pass header.d=shop.example)"],
                &[SIGNATURE],
                Some("mx.provider.test"),
                "provider_dkim_no_pass",
            ),
            (
                &[pass],
                &[],
                Some("mx.provider.test"),
                "unknown_no_correlated_signature",
            ),
            (
                &["mx.provider.test; dkim=pass header.d=other.example"],
                &[SIGNATURE],
                Some("mx.provider.test"),
                "unknown_no_correlated_signature",
            ),
            (
                &["mx.provider.test; dkim=pass header.d=shop.example header.b=ZZZZZZZZ"],
                &[SIGNATURE],
                Some("mx.provider.test"),
                "unknown_no_correlated_signature",
            ),
            (
                &[pass],
                &["v=1; d=shop.example; s=s1; h=from:list-unsubscribe; b=AbCdEf12Gh"],
                Some("mx.provider.test"),
                "unknown_incomplete_header_coverage",
            ),
            (
                &["mx.provider.test; dkim=pass header.d=shop.example"],
                &[
                    SIGNATURE,
                    "v=1; d=shop.example; s=s2; h=from; b=Other123456",
                ],
                Some("mx.provider.test"),
                "unknown_incomplete_header_coverage",
            ),
        ];
        for (results, signatures, trusted, expected) in cases {
            assert_eq!(
                one_click(&headers(results, signatures), *trusted),
                *expected,
                "{results:?} {signatures:?}"
            );
        }
        let mut repeated = headers(&[pass], &[SIGNATURE]);
        repeated
            .evidence
            .get_mut("list-unsubscribe")
            .unwrap()
            .push("<https://attacker.example/u>".into());
        assert_eq!(
            one_click(&repeated, Some("mx.provider.test")),
            "unknown_repeated_or_missing_unsubscribe_headers"
        );
        let mut missing = headers(&[pass], &[SIGNATURE]);
        missing.evidence.remove("list-unsubscribe-post");
        assert_eq!(
            one_click(&missing, Some("mx.provider.test")),
            "unknown_repeated_or_missing_unsubscribe_headers"
        );
    }
}
