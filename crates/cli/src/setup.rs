//! `condensate setup`: run by the installer to put the CLI on the user's PATH (and take it off
//! again on uninstall). Only the per-user PATH is touched, so no admin rights are needed.

use anyhow::Result;

#[cfg(windows)]
mod imp {
    use anyhow::{Context, Result};
    use std::path::Path;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_EXPAND_SZ};
    use winreg::{RegKey, RegValue};

    fn env_key() -> Result<RegKey> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
            .context("opening HKCU\\Environment")
    }

    fn read_path(key: &RegKey) -> String {
        key.get_value::<String, _>("Path").unwrap_or_default()
    }

    fn write_path(key: &RegKey, value: &str) -> Result<()> {
        let mut bytes: Vec<u8> = value.encode_utf16().chain(std::iter::once(0)).flat_map(|u| u.to_le_bytes()).collect();
        if value.is_empty() {
            bytes = vec![0, 0];
        }
        key.set_raw_value("Path", &RegValue { bytes, vtype: REG_EXPAND_SZ })?;
        broadcast_change();
        Ok(())
    }

    /// Tell Explorer (and therefore new terminals) that the environment changed.
    fn broadcast_change() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE};
        let param: Vec<u16> = "Environment\0".encode_utf16().collect();
        let mut result = 0usize;
        unsafe {
            SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, 0, param.as_ptr() as isize, SMTO_ABORTIFHUNG, 3000, &mut result);
        }
    }

    fn same(a: &str, b: &Path) -> bool {
        Path::new(a.trim().trim_end_matches('\\')) .as_os_str().eq_ignore_ascii_case(b.as_os_str())
    }

    pub fn add(dir: &Path) -> Result<bool> {
        let key = env_key()?;
        let current = read_path(&key);
        if current.split(';').any(|p| same(p, dir)) {
            return Ok(false);
        }
        let dir = dir.display().to_string();
        let updated = if current.trim().is_empty() { dir } else { format!("{};{dir}", current.trim_end_matches(';')) };
        write_path(&key, &updated)?;
        Ok(true)
    }

    pub fn remove(dir: &Path) -> Result<bool> {
        let key = env_key()?;
        let current = read_path(&key);
        let kept: Vec<&str> = current.split(';').filter(|p| !p.is_empty() && !same(p, dir)).collect();
        if kept.len() == current.split(';').filter(|p| !p.is_empty()).count() {
            return Ok(false);
        }
        write_path(&key, &kept.join(";"))?;
        Ok(true)
    }
}

fn install_dir() -> Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    Ok(exe.parent().map(|p| p.to_path_buf()).unwrap_or(exe))
}

pub fn add_to_path() -> Result<()> {
    #[cfg(windows)]
    {
        let dir = install_dir()?;
        if imp::add(&dir)? {
            println!("Added {} to your PATH. Open a new terminal to use `condensate`.", dir.display());
        } else {
            println!("{} is already on your PATH.", dir.display());
        }
    }
    #[cfg(not(windows))]
    println!("Nothing to do: add {} to your PATH yourself if needed.", install_dir()?.display());
    Ok(())
}

pub fn remove_from_path() -> Result<()> {
    #[cfg(windows)]
    {
        let dir = install_dir()?;
        if imp::remove(&dir)? {
            println!("Removed {} from your PATH.", dir.display());
        }
    }
    Ok(())
}
