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
pub mod scan;

pub use manifest::{ChunkRef, FileEntry, LaunchConfig, Manifest};

/// Directory inside every install that holds Condensate's own state. Never part of a game.
pub const META_DIR: &str = ".condensate";

/// Seconds since the Unix epoch.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
