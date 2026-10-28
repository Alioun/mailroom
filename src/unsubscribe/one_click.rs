//! The native one-click unsubscribe POST (RFC 8058).
//!
//! The host is resolved once and every answer must be public. The connection
//! goes to a checked address with certificate verification for the host. The
//! request carries exactly the one-click form: no cookies, no referrer, never
//! GET. A redirect is recorded as its status and never followed. Only the
//! status code and the URI host leave this module.
use super::safety::{BoundPost, Endpoint};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs},
    time::Duration,
};

pub(crate) trait Network: Send + Sync {
    fn resolve(&self, host: &str) -> io::Result<Vec<IpAddr>>;
    fn dial(&self, address: SocketAddr, timeout: Duration) -> io::Result<TcpStream>;
}
/// The operating system's resolver and direct TCP. Tests use a fixture network.
#[cfg_attr(test, allow(dead_code))]
pub(crate) struct System;
impl Network for System {
    fn resolve(&self, host: &str) -> io::Result<Vec<IpAddr>> {
        Ok((host, 443).to_socket_addrs()?.map(|a| a.ip()).collect())
    }
    fn dial(&self, address: SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
        TcpStream::connect_timeout(&address, timeout)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Endpoint,
    Resolve,
    Destination,
    Connect,
    Tls,
    Request,
    Response,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    /// Nothing was sent.
    NotSent { stage: Stage },
    /// The server answered with this status. A status is not unsubscribe success.
    Status { code: u16 },
    /// The request was written but no status arrived.
    Uncertain { stage: Stage },
}

const BODY: &str = "List-Unsubscribe=One-Click";
const STATUS_LINE_LIMIT: u64 = 8 * 1024;

/// Posts the one-click form to `raw`. Returns the URI host, when the URI parsed, and the outcome.
pub(crate) fn post(
    raw: &str,
    network: &dyn Network,
    tls: &native_tls::TlsConnector,
    timeout: Duration,
) -> (Option<String>, Outcome) {
    let not_sent = |stage| Outcome::NotSent { stage };
    let Ok(endpoint) = Endpoint::parse(raw) else {
        return (None, not_sent(Stage::Endpoint));
    };
    let host = endpoint.host.clone();
    let Ok(answers) = network.resolve(&endpoint.host) else {
        return (Some(host), not_sent(Stage::Resolve));
    };
    let Ok(bound) = endpoint.bind(&answers) else {
        return (Some(host), not_sent(Stage::Destination));
    };
    (Some(host), send(&bound, network, tls, timeout))
}

fn send(
    bound: &BoundPost,
    network: &dyn Network,
    tls: &native_tls::TlsConnector,
    timeout: Duration,
) -> Outcome {
    let not_sent = |stage| Outcome::NotSent { stage };
    let address = SocketAddr::new(bound.addresses()[0], 443);
    let Ok(tcp) = network.dial(address, timeout) else {
        return not_sent(Stage::Connect);
    };
    if tcp.peer_addr().map_or(true, |peer| {
        bound.verify_peer(network_peer(address, peer)).is_err()
    }) {
        return not_sent(Stage::Connect);
    }
    let _ = tcp.set_read_timeout(Some(timeout));
    let _ = tcp.set_write_timeout(Some(timeout));
    let Ok(mut stream) = tls.connect(bound.host(), tcp) else {
        return not_sent(Stage::Tls);
    };
    let target = bound.target();
    let mut request = zeroize::Zeroizing::new(Vec::with_capacity(target.len() + 256));
    request.extend_from_slice(b"POST ");
    request.extend_from_slice(target.as_bytes());
    request.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    request.extend_from_slice(bound.host().as_bytes());
    request.extend_from_slice(
        format!(
            "\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nUser-Agent: Mailroom\r\n\r\n{BODY}",
            bound.headers()[0].1,
            BODY.len()
        )
        .as_bytes(),
    );
    if stream
        .write_all(&request)
        .and_then(|_| stream.flush())
        .is_err()
    {
        return Outcome::Uncertain {
            stage: Stage::Request,
        };
    }
    match status(&mut BufReader::new(
        Read::by_ref(&mut stream).take(STATUS_LINE_LIMIT),
    )) {
        Some(code) => {
            let _ = stream.shutdown();
            Outcome::Status { code }
        }
        None => Outcome::Uncertain {
            stage: Stage::Response,
        },
    }
}

/// The address the bound destination was checked for. Tests dial a loopback
/// stand-in for it; production dials the address itself.
fn network_peer(dialed: SocketAddr, peer: SocketAddr) -> IpAddr {
    #[cfg(test)]
    if peer.ip().is_loopback() {
        return dialed.ip();
    }
    let _ = dialed;
    peer.ip()
}

fn status(reader: &mut impl BufRead) -> Option<u16> {
    let mut line = vec![];
    reader.read_until(b'\n', &mut line).ok()?;
    let line = std::str::from_utf8(&line).ok()?;
    let mut parts = line.split(' ');
    let version = parts.next()?;
    let code = parts.next()?;
    (matches!(version, "HTTP/1.1" | "HTTP/1.0") && code.len() == 3)
        .then(|| code.parse::<u16>().ok())
        .flatten()
        .filter(|c| (100..600).contains(c))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::{
        collections::BTreeMap,
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
    };

    pub(crate) const PUBLIC: &str = "93.184.216.34";

    /// Resolves names from a table and dials loopback fakes in place of public addresses.
    pub(crate) struct Fixture {
        answers: BTreeMap<String, Vec<IpAddr>>,
        routes: BTreeMap<IpAddr, SocketAddr>,
        pub(crate) dialed: Mutex<Vec<SocketAddr>>,
    }
    impl Network for Fixture {
        fn resolve(&self, host: &str) -> io::Result<Vec<IpAddr>> {
            self.answers
                .get(host)
                .cloned()
                .ok_or_else(|| io::Error::other("NXDOMAIN"))
        }
        fn dial(&self, address: SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
            self.dialed.lock().unwrap().push(address);
            TcpStream::connect_timeout(
                self.routes
                    .get(&address.ip())
                    .ok_or_else(|| io::Error::other("unroutable"))?,
                timeout,
            )
        }
    }

    pub(crate) struct Fake {
        pub(crate) address: SocketAddr,
        pub(crate) identity: crate::fake_tls::Identity,
        pub(crate) requests: Arc<Mutex<Vec<Vec<u8>>>>,
    }
    pub(crate) fn fake(reply: &'static str, delay: Duration) -> Fake {
        let identity = crate::fake_tls::identity("shop.example");
        let server = identity.clone();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(vec![]));
        let seen = requests.clone();
        thread::spawn(move || {
            while let Ok((tcp, _)) = listener.accept() {
                let mut tls = server.accept(tcp);
                let mut request = vec![0; 4096];
                let mut length = 0;
                while let Ok(n) = tls.read(&mut request[length..]) {
                    if n == 0 {
                        break;
                    }
                    length += n;
                    if request[..length].ends_with(BODY.as_bytes()) {
                        break;
                    }
                }
                request.truncate(length);
                seen.lock().unwrap().push(request);
                thread::sleep(delay);
                let _ = tls.write_all(reply.as_bytes());
                let _ = tls.flush();
            }
        });
        Fake {
            address,
            identity,
            requests,
        }
    }
    pub(crate) fn network(answers: &[(&str, &[&str])], routes: &[(&str, SocketAddr)]) -> Fixture {
        Fixture {
            answers: answers
                .iter()
                .map(|(h, a)| {
                    (
                        h.to_string(),
                        a.iter().map(|ip| ip.parse().unwrap()).collect(),
                    )
                })
                .collect(),
            routes: routes
                .iter()
                .map(|(ip, a)| (ip.parse().unwrap(), *a))
                .collect(),
            dialed: Mutex::new(vec![]),
        }
    }
    const URI: &str = "https://shop.example/remove?token=synthetic-private-token";

    #[test]
    fn posts_exactly_the_one_click_form_and_records_status_and_host_only() {
        let server = fake(
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
            Duration::ZERO,
        );
        let net = network(&[("shop.example", &[PUBLIC])], &[(PUBLIC, server.address)]);
        let (host, outcome) = post(
            URI,
            &net,
            &server.identity.connector(),
            Duration::from_secs(5),
        );
        assert_eq!(
            (host.as_deref(), &outcome),
            (Some("shop.example"), &Outcome::Status { code: 200 })
        );
        let request = String::from_utf8(server.requests.lock().unwrap()[0].clone()).unwrap();
        assert_eq!(
            request,
            "POST /remove?token=synthetic-private-token HTTP/1.1\r\nHost: shop.example\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 26\r\nConnection: close\r\nUser-Agent: Mailroom\r\n\r\nList-Unsubscribe=One-Click"
        );
        let lower = request.to_ascii_lowercase();
        assert!(!lower.contains("cookie") && !lower.contains("referer"));
        assert!(!serde_json::to_string(&outcome).unwrap().contains("token"));
        assert_eq!(
            *net.dialed.lock().unwrap(),
            vec![SocketAddr::new(PUBLIC.parse().unwrap(), 443)]
        );
    }

    #[test]
    fn statuses_and_redirects_are_recorded_and_never_followed() {
        let elsewhere = fake("HTTP/1.1 200 OK\r\n\r\n", Duration::ZERO);
        for (reply, code) in [
            ("HTTP/1.1 202 Accepted\r\n\r\n", 202),
            ("HTTP/1.1 404 Not Found\r\n\r\n", 404),
            ("HTTP/1.1 500 Oops\r\n\r\n", 500),
            (
                "HTTP/1.1 302 Found\r\nLocation: https://elsewhere.example/\r\n\r\n",
                302,
            ),
            (
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: https://10.0.0.1/\r\n\r\n",
                307,
            ),
        ] {
            let server = fake(reply, Duration::ZERO);
            let net = network(
                &[
                    ("shop.example", &[PUBLIC]),
                    ("elsewhere.example", &["93.184.216.35"]),
                ],
                &[
                    (PUBLIC, server.address),
                    ("93.184.216.35", elsewhere.address),
                ],
            );
            let (_, outcome) = post(
                URI,
                &net,
                &server.identity.connector(),
                Duration::from_secs(5),
            );
            assert_eq!(outcome, Outcome::Status { code });
            assert_eq!(
                net.dialed.lock().unwrap().len(),
                1,
                "one connection, no redirect"
            );
        }
        assert!(elsewhere.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn private_mixed_or_unresolvable_destinations_and_bad_certificates_send_nothing() {
        let server = fake("HTTP/1.1 200 OK\r\n\r\n", Duration::ZERO);
        for (answers, stage) in [
            (vec!["10.0.0.7"], Stage::Destination),
            (vec!["127.0.0.1"], Stage::Destination),
            (vec!["169.254.1.1"], Stage::Destination),
            (vec!["::ffff:93.184.216.34"], Stage::Destination),
            (vec!["fd00::1"], Stage::Destination),
            (vec![PUBLIC, "192.168.1.1"], Stage::Destination),
            (vec![], Stage::Destination),
        ] {
            let net = network(&[("shop.example", &answers)], &[(PUBLIC, server.address)]);
            assert_eq!(
                post(
                    URI,
                    &net,
                    &server.identity.connector(),
                    Duration::from_secs(5)
                )
                .1,
                Outcome::NotSent { stage }
            );
            assert!(net.dialed.lock().unwrap().is_empty());
        }
        let net = network(&[], &[]);
        assert_eq!(
            post(
                URI,
                &net,
                &server.identity.connector(),
                Duration::from_secs(5)
            )
            .1,
            Outcome::NotSent {
                stage: Stage::Resolve
            }
        );
        for bad in [
            "http://shop.example/x",
            "https://127.0.0.1/x",
            "https://user@shop.example/x",
            "https://shop.example:8443/x",
        ] {
            assert_eq!(
                post(
                    bad,
                    &net,
                    &server.identity.connector(),
                    Duration::from_secs(5)
                )
                .1,
                Outcome::NotSent {
                    stage: Stage::Endpoint
                }
            );
        }
        let net = network(&[("shop.example", &[PUBLIC])], &[(PUBLIC, server.address)]);
        let untrusted = crate::fake_tls::identity("shop.example").connector();
        assert_eq!(
            post(URI, &net, &untrusted, Duration::from_secs(5)).1,
            Outcome::NotSent { stage: Stage::Tls }
        );
        assert!(
            server.requests.lock().unwrap().is_empty(),
            "no request reached the server"
        );
    }

    #[test]
    fn a_slow_server_leaves_the_request_uncertain() {
        let server = fake("HTTP/1.1 200 OK\r\n\r\n", Duration::from_millis(1500));
        let net = network(&[("shop.example", &[PUBLIC])], &[(PUBLIC, server.address)]);
        let (_, outcome) = post(
            URI,
            &net,
            &server.identity.connector(),
            Duration::from_millis(500),
        );
        assert_eq!(
            outcome,
            Outcome::Uncertain {
                stage: Stage::Response
            }
        );
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }
}
