use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Saved login, at `<config dir>/condensate/cli.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Saved {
    pub server: Option<String>,
    pub token: Option<String>,
    pub username: Option<String>,
}

fn saved_path() -> Result<PathBuf> {
    Ok(dirs::config_dir().context("no config directory on this system")?.join("condensate").join("cli.json"))
}

impl Saved {
    pub fn load() -> Result<Self> {
        let p = saved_path()?;
        match std::fs::read(&p) {
            Ok(b) => serde_json::from_slice(&b).with_context(|| format!("corrupt {}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self) -> Result<()> {
        let p = saved_path()?;
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, serde_json::to_vec_pretty(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

/// `condensate.toml`, kept in a game's repository so pushing is a single command.
///
/// ```toml
/// game = "my-game"
///
/// [[build]]
/// platform = "windows-x64"
/// dir = "export/windows"
/// exe = "MyGame.exe"
///
/// [[build]]
/// platform = "linux-x64"
/// dir = "export/linux"
/// exe = "MyGame.x86_64"
/// ```
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectFile {
    pub game: String,
    pub server: Option<String>,
    #[serde(default, rename = "build")]
    pub builds: Vec<ProjectBuild>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectBuild {
    pub platform: String,
    pub dir: PathBuf,
    pub exe: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub workdir: Option<String>,
}

impl ProjectFile {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut project: Self = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if project.builds.is_empty() {
            bail!("{} has no [[build]] sections", path.display());
        }
        // Build dirs are relative to the file, not to wherever the command was run.
        let base = path.parent().unwrap_or(Path::new("."));
        for b in &mut project.builds {
            if b.dir.is_relative() {
                b.dir = base.join(&b.dir);
            }
        }
        Ok(project)
    }
}
