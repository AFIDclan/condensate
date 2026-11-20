pub const WINDOWS_X64: &str = "windows-x64";
pub const LINUX_X64: &str = "linux-x64";

/// Every platform a build can target.
pub const ALL: &[&str] = &[WINDOWS_X64, LINUX_X64];

pub fn is_known(platform: &str) -> bool {
    ALL.contains(&platform)
}

/// The platform this binary is running on, if Condensate supports it.
pub fn current() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some(WINDOWS_X64)
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(LINUX_X64)
    } else {
        None
    }
}
