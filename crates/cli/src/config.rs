use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// `condensate.toml`, kept in a game's repository so pushing is a single command.
///
/// ```toml
/// game = "my-game"
///
/// cover = "art/cover.png"     # optional 2:3 poster
/// banner = "art/banner.png"   # optional 16:9 banner
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
    /// Optional store artwork, uploaded by `condensate push` when it changes.
    pub cover: Option<PathBuf>,
    pub banner: Option<PathBuf>,
    #[serde(default, rename = "build")]
    pub builds: Vec<ProjectBuild>,
    #[serde(skip)]
    pub art: ProjectArt,
}

#[derive(Debug, Default)]
pub struct ProjectArt {
    pub cover: Option<PathBuf>,
    pub banner: Option<PathBuf>,
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
        let resolve = |p: &Option<PathBuf>| p.as_ref().map(|p| if p.is_relative() { base.join(p) } else { p.clone() });
        project.art = ProjectArt { cover: resolve(&project.cover), banner: resolve(&project.banner) };
        for b in &mut project.builds {
            if b.dir.is_relative() {
                b.dir = base.join(&b.dir);
            }
        }
        Ok(project)
    }
}
