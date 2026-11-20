mod auth;
mod config;
mod db;
mod error;
mod routes;
mod state;
mod storage;

use std::sync::Arc;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::state::AppState;
use crate::storage::Storage;

/// Condensate server. Configured with environment variables:
/// CONDENSATE_BIND (default 127.0.0.1:8470), CONDENSATE_DATA_DIR (default ./data),
/// CONDENSATE_NAME, CONDENSATE_DIRECT_URL.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server (default).
    Serve,
    /// Print a new single-use invite code.
    Invite,
    /// Set a new random password for a user and sign out all of their sessions.
    ResetPassword { username: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info,sqlx=warn")),
        )
        .init();

    let cli = Cli::parse();
    let config = Config::from_env()?;
    std::fs::create_dir_all(&config.data_dir)?;
    let db = db::connect(&config.data_dir).await?;

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => {
            let state = AppState::new(db, Storage::new(&config.data_dir)?, Arc::new(config.clone()));
            let app = routes::router(state);
            let listener = tokio::net::TcpListener::bind(config.bind).await?;
            tracing::info!("Condensate server listening on http://{}", config.bind);
            axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
                .with_graceful_shutdown(shutdown_signal())
                .await?;
        }
        Command::Invite => {
            let code = auth::new_invite_code();
            sqlx::query("INSERT INTO invites (code, created_by, created_at) VALUES (?, NULL, ?)")
                .bind(&code)
                .bind(db::now())
                .execute(&db)
                .await?;
            println!("{code}");
        }
        Command::ResetPassword { username } => {
            let password = auth::new_temp_password();
            let hash = auth::hash_password(password.clone()).await?;
            let res = sqlx::query("UPDATE users SET password_hash = ? WHERE username = ?")
                .bind(hash)
                .bind(&username)
                .execute(&db)
                .await?;
            if res.rows_affected() == 0 {
                bail!("no user named {username}");
            }
            sqlx::query(
                "DELETE FROM tokens WHERE kind = 'session' AND user_id = (SELECT id FROM users WHERE username = ?)",
            )
            .bind(&username)
            .execute(&db)
            .await?;
            println!("New password for {username}: {password}");
        }
    }
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
    tracing::info!("shutting down");
}
