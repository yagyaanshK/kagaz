//! TLS for TP-Link's cloud: the system's usual roots plus TP-Link's own
//! certificate authority, which their relay and some regional hosts use.

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// TP-Link's chain as the Tapo app ships it (public certificates).
const TPLINK_CA_PEM: &[u8] = include_bytes!("../../../certs/tplink-ca.pem");

/// A client configuration trusting the web roots and TP-Link's CA.
pub fn client_config() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            for cert in CertificateDer::pem_slice_iter(TPLINK_CA_PEM).flatten() {
                // A leaf or intermediate that is not a CA is skipped by the store.
                let _ = roots.add(cert);
            }
            Arc::new(
                ClientConfig::builder_with_provider(Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .with_safe_default_protocol_versions()
                .expect("ring supports the default TLS versions")
                .with_root_certificates(roots)
                .with_no_client_auth(),
            )
        })
        .clone()
}

/// An HTTP agent for TP-Link's cloud.
pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .tls_config(client_config())
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(45))
        .build()
}

/// A TLS connection to `dial:port`, verified as `server_name`.
pub fn tls_stream(
    dial: &str,
    port: u16,
    server_name: &str,
    connect_timeout: Duration,
) -> std::io::Result<StreamOwned<ClientConnection, TcpStream>> {
    let addr = (dial, port).to_socket_addrs()?.next().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no address for {dial}"),
        )
    })?;
    let sock = TcpStream::connect_timeout(&addr, connect_timeout)?;
    sock.set_nodelay(true)?;
    let name = ServerName::try_from(server_name.to_string())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?;
    let conn = ClientConnection::new(client_config(), name)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(StreamOwned::new(conn, sock))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tplink_ca_bundle_parses() {
        let n = CertificateDer::pem_slice_iter(TPLINK_CA_PEM)
            .flatten()
            .count();
        assert!(n >= 5, "{n} certificates");
        let _ = client_config();
    }
}
