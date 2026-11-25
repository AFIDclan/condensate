//! Finds the server's direct address (a port forward) and keeps checking it, so downloads, the
//! live connection and game traffic skip the main URL's proxy while the direct address works.
//! Requests fall back to the main URL on their own when it stops answering (see
//! `condensate_sdk::direct`); this loop brings the direct address back once it answers again.

use std::time::{Duration, Instant};

use condensate_sdk::Client;
use futures::future::BoxFuture;
use tauri::{AppHandle, Emitter, Manager};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

use crate::state::AppState;

/// Ask the server for its direct address this often (it changes when the home IP does).
const REFRESH: Duration = Duration::from_secs(10 * 60);
/// Check a direct address that stopped answering this often.
const RETRY: Duration = Duration::from_secs(30);

pub fn start(app: &AppHandle) {
    tauri::async_runtime::spawn(watch(app.clone()));
}

/// Whether requests go to the direct address right now.
pub fn is_direct(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let server = state.server();
    state.direct_for(&server).is_some_and(|d| d.is_up())
}

async fn watch(app: AppHandle) {
    // The server last asked about, and when to ask it again.
    let mut next: Option<(String, Instant)> = None;
    let mut was_direct = false;
    loop {
        let state = app.state::<AppState>();
        let server = state.server();
        let due = next.as_ref().map_or(true, |(s, at)| *s != server || Instant::now() >= *at);
        if due {
            if let Ok(client) = Client::new(&server, None) {
                match condensate_sdk::direct::discover(&client).await {
                    Ok(found) => {
                        *state.direct.lock().unwrap() = found.map(|d| (server.clone(), d));
                        next = Some((server.clone(), Instant::now() + REFRESH));
                    }
                    // Offline or the server is down: keep what we had and ask again soon.
                    Err(_) => next = Some((server.clone(), Instant::now() + RETRY)),
                }
            }
        } else if let Some(link) = state.direct_for(&server).filter(|d| !d.is_up()) {
            link.check().await;
        }
        let direct = is_direct(&app);
        if direct != was_direct {
            was_direct = direct;
            let _ = app.emit("connection", direct);
        }
        tokio::time::sleep(RETRY).await;
    }
}

pub type ServerSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Open a WebSocket to the server: `ws_url` with `tweak` applied. Goes to the direct address
/// when it's up, and falls back to the main URL when the direct address can't be reached.
pub async fn connect_ws(client: &Client, tweak: impl Fn(String) -> String) -> Result<ServerSocket, WsError> {
    if let Some(link) = client.direct().cloned() {
        match tokio::time::timeout(Duration::from_secs(6), open(client, &tweak)).await {
            Ok(Ok(socket)) => return Ok(socket),
            Ok(Err(WsError::Http(resp))) => return Err(WsError::Http(resp)), // the server answered
            _ => link.mark_down(),
        }
    }
    open(&client.origin(), &tweak).await
}

fn open<'a>(client: &Client, tweak: &'a dyn Fn(String) -> String) -> BoxFuture<'a, Result<ServerSocket, WsError>> {
    let url = client.ws_url().map(tweak);
    let connector = client.ws_tls().map(Connector::Rustls);
    Box::pin(async move {
        let url = url.ok_or_else(|| WsError::Io(std::io::Error::other("not logged in")))?;
        // Nagle off: game packets and voice are small and shouldn't wait for each other.
        let (socket, _) = tokio_tungstenite::connect_async_tls_with_config(url.as_str(), None, true, connector).await?;
        Ok(socket)
    })
}
