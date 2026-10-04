use std::sync::{Arc, OnceLock};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject};
use rustls::{ClientConfig, RootCertStore, ServerConfig};

use crate::{bytes::Bytes, transport::Transport};

#[derive(Clone)]
pub struct ClientTls(pub(crate) Arc<ClientConfig>);

#[derive(Clone)]
pub struct ServerTls(pub(crate) Arc<ServerConfig>);

pub fn system() -> ClientTls {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    ClientTls(
        CONFIG
            .get_or_init(|| {
                let roots = RootCertStore {
                    roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
                };
                client_config(roots)
            })
            .clone(),
    )
}

/// Custom trust replaces public roots; it never disables certificate/name verification.
pub fn client(ca_pem: &Bytes) -> Result<ClientTls, String> {
    let certificates = certificates(ca_pem)?;
    let mut roots = RootCertStore::empty();
    for certificate in certificates {
        roots.add(certificate).map_err(|error| error.to_string())?;
    }
    Ok(ClientTls(client_config(roots)))
}

fn client_config(roots: RootCertStore) -> Arc<ClientConfig> {
    let mut config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    // Share trust material and keep rustls's session-cache capacity small.
    config.resumption = rustls::client::Resumption::in_memory_sessions(32);
    Arc::new(config)
}

pub fn server(cert_pem: &Bytes, key_pem: &Bytes) -> Result<ServerTls, String> {
    let key = PrivateKeyDer::from_pem_slice(key_pem.values()).map_err(|error| error.to_string())?;
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certificates(cert_pem)?, key)
        .map_err(|error| error.to_string())?;
    config.session_storage = rustls::server::ServerSessionMemoryCache::new(32);
    config.send_tls13_tickets = 1;
    Ok(ServerTls(Arc::new(config)))
}

fn certificates(pem: &Bytes) -> Result<Vec<CertificateDer<'static>>, String> {
    let certificates = CertificateDer::pem_slice_iter(pem.values())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    if certificates.is_empty() {
        return Err("TLS PEM contains no certificates".into());
    }
    Ok(certificates)
}

pub(crate) async fn connect(
    stream: tokio::net::TcpStream,
    host: &str,
    config: &ClientTls,
) -> Result<Transport, String> {
    let name = ServerName::try_from(host.to_owned()).map_err(|error| error.to_string())?;
    let stream = tokio_rustls::TlsConnector::from(config.0.clone())
        .connect(name, stream)
        .await
        .map_err(|error| error.to_string())?;
    Ok(Transport::Tls(Box::new(stream.into())))
}

pub(crate) async fn accept(
    stream: tokio::net::TcpStream,
    config: Option<&ServerTls>,
    millis: i64,
) -> Result<Transport, String> {
    let Some(config) = config else {
        return Ok(Transport::Tcp(stream));
    };
    let stream = tokio::time::timeout(
        crate::task::positive_duration(millis)?,
        tokio_rustls::TlsAcceptor::from(config.0.clone()).accept(stream),
    )
    .await
    .map_err(|_| "TLS handshake timed out".to_owned())?
    .map_err(|error| error.to_string())?;
    Ok(Transport::Tls(Box::new(stream.into())))
}

impl std::fmt::Debug for ClientTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClientTls(<resource>)")
    }
}

impl std::fmt::Debug for ServerTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServerTls(<resource>)")
    }
}
