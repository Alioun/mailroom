//! Local TLS identities for fake servers in tests.
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
};
use std::{net::TcpStream, sync::Arc, time::Duration};

#[derive(Clone)]
pub(crate) struct Identity {
    pub der: Vec<u8>,
    pub config: Arc<ServerConfig>,
}
/// A fresh self-signed certificate for `name`.
pub(crate) fn identity(name: &str) -> Identity {
    let cert = rcgen::generate_simple_self_signed(vec![name.to_owned()]).unwrap();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()));
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert.cert.der().clone()], key)
        .unwrap();
    Identity {
        der: cert.cert.der().to_vec(),
        config: Arc::new(config),
    }
}
impl Identity {
    pub(crate) fn accept(&self, tcp: TcpStream) -> StreamOwned<ServerConnection, TcpStream> {
        tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        tcp.set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        StreamOwned::new(ServerConnection::new(self.config.clone()).unwrap(), tcp)
    }
    /// A client that trusts only this certificate.
    pub(crate) fn connector(&self) -> native_tls::TlsConnector {
        native_tls::TlsConnector::builder()
            .disable_built_in_roots(true)
            .add_root_certificate(native_tls::Certificate::from_der(&self.der).unwrap())
            .build()
            .unwrap()
    }
}
