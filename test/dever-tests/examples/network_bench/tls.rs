use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::config::HttpVersion;

pub trait Stream: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite> Stream for T {}

pub type BoxedStream = Box<dyn Stream + Unpin + Send>;

pub fn runtime_server(
    cert_path: &str,
    key_path: &str,
) -> Result<dever_runtime::tls::ServerTls, String> {
    let certificate = std::fs::read(cert_path)
        .map_err(|error| format!("cannot read certificate {cert_path}: {error}"))?;
    let key = std::fs::read(key_path)
        .map_err(|error| format!("cannot read private key {key_path}: {error}"))?;
    dever_runtime::tls::server(
        &dever_runtime::bytes::Bytes::new(certificate),
        &dever_runtime::bytes::Bytes::new(key),
    )
}

pub fn server(
    cert_path: &str,
    key_path: &str,
    http_version: HttpVersion,
) -> Result<Arc<ServerConfig>, String> {
    let certificates = certificates(cert_path)?;
    let key_bytes = std::fs::read(key_path)
        .map_err(|error| format!("cannot read private key {key_path}: {error}"))?;
    let key = PrivateKeyDer::from_pem_slice(&key_bytes).map_err(|error| error.to_string())?;
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certificates, key)
        .map_err(|error| error.to_string())?;
    config.session_storage = rustls::server::ServerSessionMemoryCache::new(32);
    config.send_tls13_tickets = 1;
    config.alpn_protocols = vec![http_version.alpn().to_vec()];
    Ok(Arc::new(config))
}

pub fn client(ca_path: &str, http_version: HttpVersion) -> Result<Arc<ClientConfig>, String> {
    let mut roots = RootCertStore::empty();
    for certificate in certificates(ca_path)? {
        roots.add(certificate).map_err(|error| error.to_string())?;
    }
    let mut config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.resumption = rustls::client::Resumption::in_memory_sessions(32);
    config.alpn_protocols = vec![http_version.alpn().to_vec()];
    Ok(Arc::new(config))
}

pub async fn connect(
    port: u16,
    timeout: Duration,
    tls: Option<&Arc<ClientConfig>>,
    http_version: HttpVersion,
) -> Result<BoxedStream, String> {
    let tcp = tokio::time::timeout(timeout, TcpStream::connect(("127.0.0.1", port)))
        .await
        .map_err(|_| "TCP connection timed out".to_owned())?
        .map_err(|error| error.to_string())?;
    tcp.set_nodelay(true).map_err(|error| error.to_string())?;
    let Some(config) = tls else {
        return Ok(Box::new(tcp));
    };
    let name = ServerName::try_from("localhost").map_err(|error| error.to_string())?;
    let stream = tokio::time::timeout(
        timeout,
        TlsConnector::from(config.clone()).connect(name, tcp),
    )
    .await
    .map_err(|_| "TLS handshake timed out".to_owned())?
    .map_err(|error| error.to_string())?;
    verify_alpn(stream.get_ref().1.alpn_protocol(), http_version)?;
    Ok(Box::new(stream))
}

pub async fn accept(
    tcp: TcpStream,
    timeout: Duration,
    tls: Option<&Arc<ServerConfig>>,
    http_version: HttpVersion,
) -> Result<BoxedStream, String> {
    let Some(config) = tls else {
        return Ok(Box::new(tcp));
    };
    let stream = tokio::time::timeout(timeout, TlsAcceptor::from(config.clone()).accept(tcp))
        .await
        .map_err(|_| "TLS handshake timed out".to_owned())?
        .map_err(|error| error.to_string())?;
    verify_alpn(stream.get_ref().1.alpn_protocol(), http_version)?;
    Ok(Box::new(stream))
}

fn verify_alpn(negotiated: Option<&[u8]>, http_version: HttpVersion) -> Result<(), String> {
    if http_version == HttpVersion::H2 && negotiated != Some(http_version.alpn()) {
        return Err("TLS peer did not negotiate h2".into());
    }
    Ok(())
}

fn certificates(path: impl AsRef<Path>) -> Result<Vec<CertificateDer<'static>>, String> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)
        .map_err(|error| format!("cannot read certificate {}: {error}", path.display()))?;
    let certificates = CertificateDer::pem_slice_iter(&bytes)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    if certificates.is_empty() {
        return Err(format!(
            "certificate {} contains no certificates",
            path.display()
        ));
    }
    Ok(certificates)
}
