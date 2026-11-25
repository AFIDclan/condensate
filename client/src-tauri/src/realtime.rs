//! The live connection to the server: presence and chat events arrive here and are forwarded
//! to the UI as `ws-event`; what we're playing goes the other way. Reconnects on its own.

use std::time::Duration;

use condensate_core::api::{WsCommand, WsEvent};
use futures::{SinkExt, StreamExt};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio_tungstenite::tungstenite::Message;

use crate::state::AppState;

/// (Re)start the connection for the current login. Safe to call repeatedly.
pub fn start(app: &AppHandle) {
    let state = app.state::<AppState>();
    let (tx, rx) = unbounded_channel();
    let generation = {
        let mut rt = state.realtime.lock().unwrap();
        let gen = rt.as_ref().map_or(1, |(g, _)| g + 1);
        *rt = Some((gen, tx));
        gen
    };
    tauri::async_runtime::spawn(run(app.clone(), generation, rx));
}

/// Close the connection (on logout).
pub fn stop(app: &AppHandle) {
    *app.state::<AppState>().realtime.lock().unwrap() = None;
}

/// Queue a command for the server; dropped silently when offline.
pub fn send(app: &AppHandle, cmd: WsCommand) {
    if let Some((_, tx)) = app.state::<AppState>().realtime.lock().unwrap().as_ref() {
        let _ = tx.send(cmd);
    }
}

fn current(app: &AppHandle, generation: u64) -> bool {
    app.state::<AppState>().realtime.lock().unwrap().as_ref().is_some_and(|(g, _)| *g == generation)
}

async fn run(app: AppHandle, generation: u64, mut outbox: UnboundedReceiver<WsCommand>) {
    let mut backoff = Duration::from_secs(1);
    while current(&app, generation) {
        let Ok(client) = app.state::<AppState>().client() else { return };
        match crate::route::connect_ws(&client, |url| url).await {
            Ok(socket) => {
                backoff = Duration::from_secs(1);
                let _ = app.emit("realtime", true);
                let (mut sink, mut stream) = socket.split();
                // Re-announce what we're playing after a reconnect.
                let playing = app.state::<AppState>().running.lock().unwrap().keys().next().cloned();
                if let Ok(text) = serde_json::to_string(&WsCommand::Playing { game: playing }) {
                    let _ = sink.send(Message::Text(text.into())).await;
                }
                let mut ping = tokio::time::interval(Duration::from_secs(30));
                loop {
                    tokio::select! {
                        msg = stream.next() => match msg {
                            Some(Ok(Message::Text(text))) => {
                                if let Ok(event) = serde_json::from_str::<WsEvent>(text.as_str()) {
                                    let _ = app.emit("ws-event", event);
                                }
                            }
                            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                            Some(Ok(_)) => {}
                        },
                        cmd = outbox.recv() => match cmd {
                            Some(cmd) => {
                                let Ok(text) = serde_json::to_string(&cmd) else { continue };
                                if sink.send(Message::Text(text.into())).await.is_err() { break; }
                            }
                            None => return, // logged out
                        },
                        _ = ping.tick() => {
                            let text = serde_json::to_string(&WsCommand::Ping).unwrap();
                            if sink.send(Message::Text(text.into())).await.is_err() { break; }
                        }
                    }
                }
                let _ = app.emit("realtime", false);
            }
            Err(_) => {}
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}
