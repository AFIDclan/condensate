use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use condensate_sdk::Client;
use serde::{Deserialize, Serialize};

use crate::commands::CmdResult;

pub const DEFAULT_SERVER: &str = "https://oort-cloud.tail4b4a9d.ts.net";

/// Saved to `<app config dir>/settings.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub server: String,
    pub token: Option<String>,
    pub username: Option<String>,
    /// Where games are installed. Defaults to `<app local data>/games`.
    pub library_dir: Option<PathBuf>,
    /// Where this app was launched from, so the CLI can open it to log in. An AppImage has no
    /// fixed install folder for the CLI to look in.
    pub app_path: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { server: DEFAULT_SERVER.into(), token: None, username: None, library_dir: None, app_path: None }
    }
}

/// An install or update in progress.
#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub slug: String,
    /// checking | downloading | installing
    pub phase: String,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub reused_bytes: u64,
}

pub struct AppState {
    pub settings: Mutex<Settings>,
    settings_path: PathBuf,
    default_library: PathBuf,
    pub jobs: Mutex<HashMap<String, Job>>,
    /// slug -> process id of running games.
    pub running: Mutex<HashMap<String, u32>>,
    /// The live connection's generation and outbox (see realtime.rs).
    pub realtime: Mutex<Option<(u64, tokio::sync::mpsc::UnboundedSender<condensate_core::api::WsCommand>)>>,
}

impl AppState {
    pub fn load(config_dir: &Path, data_dir: &Path) -> Self {
        let settings_path = config_dir.join("settings.json");
        let settings = std::fs::read(&settings_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            settings: Mutex::new(settings),
            settings_path,
            default_library: data_dir.join("games"),
            jobs: Mutex::new(HashMap::new()),
            running: Mutex::new(HashMap::new()),
            realtime: Mutex::new(None),
        }
    }

    /// Re-read settings from disk: the CLI shares this file and may have logged in or out.
    pub fn reload(&self) {
        if let Some(fresh) = std::fs::read(&self.settings_path).ok().and_then(|b| serde_json::from_slice::<Settings>(&b).ok()) {
            *self.settings.lock().unwrap() = fresh;
        }
    }

    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) -> CmdResult<()> {
        let mut s = self.settings.lock().unwrap();
        f(&mut s);
        if let Some(dir) = self.settings_path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_vec_pretty(&*s).map_err(|e| e.to_string())?;
        std::fs::write(&self.settings_path, json).map_err(|e| e.to_string())
    }

    /// Record where the app lives (see `Settings::app_path`).
    pub fn record_app_path(&self) {
        let path = std::env::var_os("APPIMAGE").map(PathBuf::from).or_else(|| std::env::current_exe().ok());
        let changed = path.is_some() && self.settings.lock().unwrap().app_path != path;
        if changed {
            let _ = self.update_settings(|s| s.app_path = path);
        }
    }

    pub fn server(&self) -> String {
        self.settings.lock().unwrap().server.clone()
    }

    /// Authenticated client, or an error the UI treats as "please log in".
    pub fn client(&self) -> CmdResult<Client> {
        let s = self.settings.lock().unwrap();
        let token = s.token.clone().ok_or_else(|| crate::commands::NOT_LOGGED_IN.to_string())?;
        Client::new(&s.server, Some(token)).map_err(|e| e.to_string())
    }

    pub fn library_dir(&self) -> PathBuf {
        self.settings.lock().unwrap().library_dir.clone().unwrap_or_else(|| self.default_library.clone())
    }

    pub fn game_dir(&self, slug: &str) -> PathBuf {
        self.library_dir().join(slug)
    }
}

/// Move every installed game from one games folder to another (rename when possible, copy
/// across drives otherwise). Folders that aren't Condensate installs are left alone.
pub fn move_games(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(to)?;
    let Ok(entries) = std::fs::read_dir(from) else { return Ok(()) };
    for entry in entries.flatten() {
        let src = entry.path();
        if !src.join(condensate_core::META_DIR).join("installed.json").is_file() {
            continue;
        }
        let dest = to.join(entry.file_name());
        if dest.exists() {
            anyhow::bail!("{} already exists", dest.display());
        }
        if std::fs::rename(&src, &dest).is_err() {
            copy_dir(&src, &dest)?;
            std::fs::remove_dir_all(&src)?;
        }
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}
