//! The direct address: the same API served over TLS on a port forward, so downloads and game
//! traffic skip the Tailscale Funnel relay.
//!
//! There's no domain, so the certificate is self-signed (made on first start and kept in
//! `<data>/direct/`). The server advertises the address and the certificate's BLAKE3
//! fingerprint in `GET /api/v1/server`, which clients fetch over the Funnel's real TLS, and
//! clients accept exactly that certificate on the direct address. Clients fall back to the
//! Funnel whenever the direct address doesn't answer.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use axum::Router;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;

use crate::config::Config;

/// What `GET /api/v1/server` advertises.
#[derive(Default)]
pub struct DirectInfo {
    /// Hex BLAKE3 of the certificate (DER); None when the direct port is off.
    pub cert: Option<String>,
    url: RwLock<Option<String>>,
}

impl DirectInfo {
    pub fn url(&self) -> Option<String> {
        self.url.read().unwrap().clone()
    }
}

/// The certificate and key, made on first use.
struct Identity {
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

fn identity(data_dir: &Path) -> Result<Identity> {
    let dir = data_dir.join("direct");
    let (cert_path, key_path) = (dir.join("cert.der"), dir.join("key.der"));
    if !cert_path.is_file() || !key_path.is_file() {
        std::fs::create_dir_all(&dir)?;
        let made = rcgen::generate_simple_self_signed(vec!["condensate".to_string()])
            .context("couldn't make the direct certificate")?;
        std::fs::write(&key_path, made.key_pair.serialize_der())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::write(&cert_path, made.cert.der())?;
        tracing::info!("made a certificate for the direct address in {}", dir.display());
    }
    Ok(Identity {
        cert: CertificateDer::from(std::fs::read(&cert_path)?),
        key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(std::fs::read(&key_path)?)),
    })
}

/// Prepare the direct address from the config. Returns what to advertise and, when the port
/// is on, a function that starts serving once the router exists.
pub fn prepare(config: &Config) -> Result<(Arc<DirectInfo>, Option<Starter>)> {
    let Some(port) = config.direct_port else {
        return Ok((Arc::new(DirectInfo::default()), None));
    };
    let id = identity(&config.data_dir)?;
    let info = Arc::new(DirectInfo {
        cert: Some(blake3::hash(id.cert.as_ref()).to_hex().to_string()),
        url: RwLock::new(config.direct_url.clone()),
    });
    let starter = Starter { port, id, info: info.clone(), fixed_url: config.direct_url.is_some() };
    Ok((info, Some(starter)))
}

pub struct Starter {
    port: u16,
    id: Identity,
    info: Arc<DirectInfo>,
    fixed_url: bool,
}

impl Starter {
    /// Listen on the direct port and serve `app` there, in the background.
    pub async fn start(self, app: Router) -> Result<()> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut tls = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(vec![self.id.cert], self.id.key)
            .context("the direct certificate is unusable")?;
        // HTTP/1.1 only: WebSockets over HTTP/2 need extended CONNECT, which clients lack.
        tls.alpn_protocols = vec![b"http/1.1".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(tls));

        let addr = SocketAddr::from(([0, 0, 0, 0], self.port));
        let tcp = TcpListener::bind(addr).await.with_context(|| format!("couldn't listen on {addr}"))?;
        tracing::info!("direct address listening on https://{addr}");

        // Handshakes run in their own tasks so a slow or silent client can't hold up the rest.
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            loop {
                let (stream, peer) = match tcp.accept().await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("direct accept failed: {e}");
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        continue;
                    }
                };
                let (acceptor, tx) = (acceptor.clone(), tx.clone());
                tokio::spawn(async move {
                    let _ = stream.set_nodelay(true); // game packets shouldn't wait
                    if let Ok(Ok(tls)) = tokio::time::timeout(Duration::from_secs(10), acceptor.accept(stream)).await {
                        let _ = tx.send((tls, peer)).await;
                    }
                });
            }
        });
        tokio::spawn(async move {
            if let Err(e) = axum::serve(TlsListener { rx, addr }, app).await {
                tracing::error!("direct address stopped: {e}");
            }
        });

        if !self.fixed_url {
            tokio::spawn(track_public_ip(self.info, self.port));
        }
        Ok(())
    }
}

struct TlsListener {
    rx: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    addr: SocketAddr,
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(conn) => conn,
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        Ok(self.addr)
    }
}

/// Keep the advertised URL pointing at the current public IP (home connections change it now
/// and then). Checks every 10 minutes; keeps the last known address when the lookup fails.
async fn track_public_ip(info: Arc<DirectInfo>, port: u16) {
    let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().expect("http client");
    loop {
        match public_ip(&http).await {
            Ok(ip) => {
                let url = format!("https://{ip}:{port}");
                let mut current = info.url.write().unwrap();
                if current.as_deref() != Some(url.as_str()) {
                    tracing::info!("direct address is now {url}");
                    *current = Some(url);
                }
            }
            Err(e) => tracing::warn!("couldn't look up the public IP: {e:#}"),
        }
        tokio::time::sleep(Duration::from_secs(600)).await;
    }
}

async fn public_ip(http: &reqwest::Client) -> Result<std::net::Ipv4Addr> {
    let text = http.get("https://api.ipify.org").send().await?.error_for_status()?.text().await?;
    text.trim().parse().with_context(|| format!("unexpected answer {text:?}"))
}
