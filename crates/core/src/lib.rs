//! Shared code for every part of Condensate.
//!
//! A game *build* (one platform of one version) is described by a [`Manifest`]: the list of
//! files, and for each file the ordered list of content-defined chunks it is made of. Chunks are
//! identified by the BLAKE3 hash of their uncompressed bytes, stored zstd-compressed on the
//! server, and shared between every file, version and game. Updating a game means downloading
//! only the chunks the client cannot already find in its current install.

pub mod api;
pub mod chunk;
pub mod install;
pub mod manifest;
pub mod platform;
pub mod play;
pub mod scan;

pub use manifest::{ChunkRef, FileEntry, LaunchConfig, Manifest};

/// Directory inside every install that holds Condensate's own state. Never part of a game.
pub const META_DIR: &str = ".condensate";

/// (year, month, day, hour, minute) in UTC for a Unix timestamp.
pub fn utc_parts(ts: i64) -> (i64, i64, i64, i64, i64) {
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, secs / 3600, secs / 60 % 60)
}

/// Seconds since the Unix epoch.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
