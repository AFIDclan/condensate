use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use condensate_sdk::Client;
use serde::{Deserialize, Serialize};

use crate::commands::CmdResult;

pub const DEFAULT_SERVER: &str = "https://oort-cloud.tail4b4a9d.ts.net";

/// Saved to `<app config dir>/settings.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub server: String,
    pub token: Option<String>,
    pub username: Option<String>,
    /// Where games are installed. Defaults to `<app local data>/games`.
    pub library_dir: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { server: DEFAULT_SERVER.into(), token: None, username: None, library_dir: None }
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
