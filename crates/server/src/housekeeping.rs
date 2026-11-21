//! Maintenance jobs, run from the command line (and nightly by a systemd timer).

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use condensate_core::Manifest;
use sqlx::SqlitePool;

use crate::storage::Storage;

/// Files younger than this are never collected: they may belong to a push still in progress.
const GRACE: Duration = Duration::from_secs(24 * 3600);

fn old_enough(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age > GRACE)
}

/// Delete chunks no build references and images no game or user uses.
pub async fn gc(db: &SqlitePool, storage: &Storage, dry_run: bool) -> Result<()> {
    let mut chunks = HashSet::new();
    let manifests: Vec<String> = sqlx::query_scalar("SELECT manifest FROM builds").fetch_all(db).await?;
    for text in &manifests {
        let m: Manifest = serde_json::from_str(text).context("corrupt manifest in database")?;
        chunks.extend(m.files.into_iter().flat_map(|f| f.chunks.into_iter().map(|c| c.hash)));
    }
    let images: HashSet<String> = sqlx::query_scalar(
        "SELECT cover_image FROM games WHERE cover_image IS NOT NULL
         UNION SELECT banner_image FROM games WHERE banner_image IS NOT NULL
         UNION SELECT avatar_image FROM users WHERE avatar_image IS NOT NULL",
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .collect();

    let (mut n, mut bytes) = (0u64, 0u64);
    let mut sweep = |path: &Path, keep: bool| {
        if keep || !old_enough(path) {
            return;
        }
        n += 1;
        bytes += std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if !dry_run {
            let _ = std::fs::remove_file(path);
        }
    };
    for shard in std::fs::read_dir(storage.chunks_dir())?.flatten() {
        for entry in std::fs::read_dir(shard.path())?.flatten() {
            let path = entry.path();
            let hash = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            sweep(&path, chunks.contains(hash));
        }
    }
    for entry in std::fs::read_dir(storage.images_dir())?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        sweep(&entry.path(), images.contains(&name));
    }
    println!(
        "{} {n} unreferenced files ({:.1} MB); {} chunks still referenced by {} builds",
        if dry_run { "Would delete" } else { "Deleted" },
        bytes as f64 / 1_048_576.0,
        chunks.len(),
        manifests.len()
    );
    Ok(())
}

/// Consistent snapshot of the database via `VACUUM INTO`, keeping the newest `keep`.
pub async fn backup(db: &SqlitePool, data_dir: &Path, keep: usize) -> Result<()> {
    let dir = data_dir.join("backups");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("condensate-{}.db", condensate_core::unix_now()));
    sqlx::query("VACUUM INTO ?").bind(path.to_string_lossy().to_string()).execute(db).await?;
    println!("Backed up to {}", path.display());

    let mut backups: Vec<_> = std::fs::read_dir(&dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "db"))
        .collect();
    backups.sort();
    let excess = backups.len().saturating_sub(keep.max(1));
    for old in &backups[..excess] {
        std::fs::remove_file(old)?;
    }
    Ok(())
}
