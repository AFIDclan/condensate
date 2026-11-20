use std::path::Path;
use std::process::{Child, Command};

use anyhow::{anyhow, Context, Result};
use condensate_core::install::read_installed;
use condensate_core::manifest::native_path;

/// Start the game installed in `dir`.
pub fn launch(dir: &Path) -> Result<Child> {
    let state = read_installed(dir)?.ok_or_else(|| anyhow!("no game is installed in {}", dir.display()))?;
    let exe = native_path(dir, &state.launch.exe);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&exe).with_context(|| format!("missing {}", exe.display()))?.permissions();
        if perm.mode() & 0o111 == 0 {
            perm.set_mode(perm.mode() | 0o755);
            std::fs::set_permissions(&exe, perm)?;
        }
    }

    let workdir = match state.launch.workdir.as_deref() {
        Some(".") => dir.to_path_buf(),
        Some(w) => native_path(dir, w),
        None => exe.parent().unwrap_or(dir).to_path_buf(),
    };
    Command::new(&exe)
        .args(&state.launch.args)
        .current_dir(&workdir)
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))
}
