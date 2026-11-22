//! The login shared by the desktop app and the CLI.
//!
//! Both read and write `<config dir>/com.condensate.app/settings.json` (the app's own settings
//! file), so logging in or out in either one does it for both. Keys this module doesn't know
//! about (like the app's games folder) are preserved.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{Map, Value};

/// The desktop app's identifier; also the name of its config folder.
pub const APP_ID: &str = "com.condensate.app";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredLogin {
    pub server: Option<String>,
    pub token: Option<String>,
    pub username: Option<String>,
}

pub fn settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(APP_ID).join("settings.json"))
}

fn read_object() -> Map<String, Value> {
    settings_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}

pub fn load() -> StoredLogin {
    let m = read_object();
    let s = |k: &str| m.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()).map(String::from);
    StoredLogin { server: s("server"), token: s("token"), username: s("username") }
}

/// Where the desktop app was last launched from (it records this on every start).
pub fn app_path() -> Option<PathBuf> {
    read_object().get("app_path").and_then(Value::as_str).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The running app's local multiplayer API, as (`ws://` URL, token), from the file it writes at
/// startup (`condensate_core::play::GAME_API_FILE`).
pub fn game_api() -> Option<(String, String)> {
    let path = dirs::config_dir()?.join(APP_ID).join(condensate_core::play::GAME_API_FILE);
    let v: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    Some((v.get("api")?.as_str()?.to_string(), v.get("token")?.as_str()?.to_string()))
}

pub fn save(login: &StoredLogin) -> Result<()> {
    let path = settings_path().context("no config directory on this system")?;
    let mut m = read_object();
    let mut set = |k: &str, v: &Option<String>| {
        m.insert(k.to_string(), v.clone().map(Value::String).unwrap_or(Value::Null));
    };
    set("server", &login.server);
    set("token", &login.token);
    set("username", &login.username);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&Value::Object(m))?)?;
    std::fs::rename(&tmp, &path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}
