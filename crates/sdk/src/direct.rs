//! The server's direct address (a port forward), used in place of the main URL while it works.
//!
//! The server advertises the address and its self-signed certificate's BLAKE3 fingerprint in
//! `GET /api/v1/server`. That response comes over the main URL's normal TLS, so pinning the
//! fingerprint is as safe as the main URL. See `crates/server/src/direct.rs`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

use crate::client::Client;

/// A working (or recently working) direct route. Shared between clients; any of them can mark
/// it down when a request can't get through, and they all go back to the main URL.
pub struct DirectLink {
    pub(crate) http: reqwest::Client,
    pub(crate) base: String,
    tls: Arc<rustls::ClientConfig>,
    up: AtomicBool,
}

impl DirectLink {
    /// `cert` is the hex BLAKE3 fingerprint from the server info.
    pub fn new(url: &str, cert: &str) -> Result<Self> {
        let mut pin = [0u8; 32];
        let bytes = (0..cert.len())
            .step_by(2)
            .map(|i| cert.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
            .collect::<Option<Vec<u8>>>();
        match bytes {
            Some(b) if b.len() == 32 => pin.copy_from_slice(&b),
            _ => bail!("the server sent a malformed certificate fingerprint"),
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let tls = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Pinned { pin, provider }))
            .with_no_client_auth();
        let http = reqwest::Client::builder()
            .user_agent(concat!("condensate/", env!("CARGO_PKG_VERSION")))
            .use_preconfigured_tls(tls.clone())
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(60))
            .tcp_nodelay(true)
            .build()?;
        Ok(Self { http, base: url.trim_end_matches('/').to_string(), tls: Arc::new(tls), up: AtomicBool::new(false) })
    }

    pub fn url(&self) -> &str {
        &self.base
    }

    /// TLS settings for WebSocket connections to the direct address.
    pub fn tls(&self) -> Arc<rustls::ClientConfig> {
        self.tls.clone()
    }

    pub fn is_up(&self) -> bool {
        self.up.load(Ordering::Relaxed)
    }

    /// Stop using the direct address until a `check` succeeds.
    pub fn mark_down(&self) {
        self.up.store(false, Ordering::Relaxed);
    }

    /// Try the direct address; marks it up or down. Takes up to about 5 seconds.
    pub async fn check(&self) -> bool {
        let ok = async {
            let resp = self.http.get(format!("{}/api/v1/server", self.base)).timeout(Duration::from_secs(5)).send().await?;
            resp.error_for_status()?.json::<condensate_core::api::ServerInfo>().await
        }
        .await
        .is_ok();
        self.up.store(ok, Ordering::Relaxed);
        ok
    }
}

/// Ask the server for its direct address and check whether it works from here (see
/// [`DirectLink::is_up`]). None when the server has none; the main URL works either way.
pub async fn discover(client: &Client) -> Result<Option<Arc<DirectLink>>> {
    let info = client.origin().server_info().await.context("couldn't get the server info")?;
    let (Some(url), Some(cert)) = (info.direct_url, info.direct_cert) else { return Ok(None) };
    let link = DirectLink::new(&url, &cert)?;
    link.check().await;
    Ok(Some(Arc::new(link)))
}

/// Accepts exactly the advertised certificate. The host name doesn't matter (the address is a
/// bare IP); signatures are still checked, so the server must hold the certificate's key.
#[derive(Debug)]
struct Pinned {
    pin: [u8; 32],
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if blake3::hash(end_entity.as_ref()).as_bytes() == &self.pin {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("the direct address presented a different certificate than the server advertised".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_fingerprints() {
        assert!(DirectLink::new("https://127.0.0.1:1", "zz").is_err());
        assert!(DirectLink::new("https://127.0.0.1:1", &"ab".repeat(31)).is_err());
        assert!(DirectLink::new("https://127.0.0.1:1", &"ab".repeat(32)).is_ok());
    }

    /// A TLS server with a fresh self-signed certificate that answers every request with a
    /// server info body. Returns its URL and the certificate's fingerprint.
    async fn tls_server() -> (String, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let made = rcgen::generate_simple_self_signed(vec!["condensate".to_string()]).unwrap();
        let cert = made.cert.der().clone();
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(made.key_pair.serialize_der().into());
        let fingerprint = blake3::hash(cert.as_ref()).to_hex().to_string();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(stream).await else { return };
                    let mut buf = [0u8; 4096];
                    let _ = tls.read(&mut buf).await;
                    let body = r#"{"name":"t","server_version":"0","direct_url":null,"needs_invite":true}"#;
                    let head = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
                    let _ = tls.write_all((head + body).as_bytes()).await;
                    let _ = tls.shutdown().await;
                });
            }
        });
        (url, fingerprint)
    }

    #[tokio::test]
    async fn accepts_only_the_pinned_certificate() {
        let (url, fingerprint) = tls_server().await;
        let right = DirectLink::new(&url, &fingerprint).unwrap();
        assert!(right.check().await, "the advertised certificate is accepted");
        assert!(right.is_up());
        let wrong = DirectLink::new(&url, &blake3::hash(b"another certificate").to_hex()).unwrap();
        assert!(!wrong.check().await, "any other certificate is refused");
        assert!(!wrong.is_up());
    }
}
