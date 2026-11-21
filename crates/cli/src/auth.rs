//! Where the CLI's login comes from: the login shared with the desktop app, an older CLI-only
//! login file, or (interactively) by opening the app and waiting for the user to log in there.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use condensate_sdk::login_store::{self, StoredLogin};

/// The shared login, falling back to the pre-0.2 `condensate/cli.json`.
pub fn current_login() -> StoredLogin {
    let shared = login_store::load();
    if shared.token.is_some() {
        return shared;
    }
    legacy_login().filter(|l| l.token.is_some()).unwrap_or(shared)
}

fn legacy_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("condensate").join("cli.json"))
}

fn legacy_login() -> Option<StoredLogin> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(legacy_path()?).ok()?).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(String::from);
    Some(StoredLogin { server: s("server"), token: s("token"), username: s("username") })
}

/// Forget the old CLI-only login so it can't shadow the shared one.
pub fn clear_legacy() {
    if let Some(p) = legacy_path() {
        let _ = std::fs::remove_file(p);
    }
}

pub fn same_server(a: &str, b: &str) -> bool {
    a.trim_end_matches('/').eq_ignore_ascii_case(b.trim_end_matches('/'))
}

/// The desktop app: installed alongside this CLI, or wherever it was last launched from (an
/// AppImage, whose CLI copy lives in ~/.local/bin).
fn find_app() -> Option<PathBuf> {
    let beside = std::env::current_exe().ok().and_then(|exe| {
        let dir = exe.parent()?.to_path_buf();
        let names: &[&str] = if cfg!(windows) {
            &["Condensate.exe", "condensate-client.exe"]
        } else {
            &["condensate-client", "Condensate", "condensate-app"]
        };
        names.iter().map(|n| dir.join(n)).find(|p| p.is_file())
    });
    beside.or_else(|| login_store::app_path().filter(|p| p.is_file()))
}

/// Open the desktop app and wait (up to 5 minutes) for the user to log in there.
/// Returns `None` when the app isn't installed next to the CLI.
pub async fn login_with_app() -> Result<Option<StoredLogin>> {
    let Some(app) = find_app() else { return Ok(None) };
    eprintln!("Not logged in. Opening Condensate. Log in there and this command will continue.");
    eprintln!("(Or press Ctrl+C and run `condensate login <username>`.)");
    std::process::Command::new(&app).spawn().with_context(|| format!("starting {}", app.display()))?;

    let deadline = Instant::now() + Duration::from_secs(300);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(800)).await;
        let login = login_store::load();
        if login.token.is_some() && login.server.is_some() {
            eprintln!("Logged in as {}.", login.username.as_deref().unwrap_or("?"));
            return Ok(Some(login));
        }
    }
    anyhow::bail!("timed out waiting for you to log in to the Condensate app")
}
