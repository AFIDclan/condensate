use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use condensate_core::chunk::{decompress_verified, hash_bytes};
use condensate_core::install::{self, chunk_cache_dir, InstalledState, Plan};
use condensate_core::{unix_now, Manifest};
use futures::{StreamExt, TryStreamExt};
use serde::Serialize;

use crate::{with_retries, Client};

const DOWNLOAD_CONCURRENCY: usize = 8;

pub struct PullOptions {
    pub game: String,
    pub platform: String,
    pub dir: PathBuf,
    /// Re-check every file on disk instead of trusting the recorded install state.
    pub repair: bool,
}

#[derive(Debug, Clone)]
pub enum PullEvent {
    Checking,
    Planned { files_to_write: usize, files_to_delete: usize, download_bytes: u64, reused_bytes: u64 },
    Downloading { done_chunks: usize, total_chunks: usize, done_bytes: u64, total_bytes: u64 },
    Applying,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullReport {
    pub game: String,
    pub platform: String,
    pub from_version: Option<String>,
    pub to_version: String,
    pub changelog: String,
    pub up_to_date: bool,
    pub downloaded_chunks: usize,
    pub downloaded_bytes: u64,
    pub reused_bytes: u64,
    pub files_written: usize,
    pub files_deleted: usize,
}

/// Install a game into `dir`, or update the install that is already there.
pub async fn pull(client: &Client, opts: PullOptions, on_event: &(dyn Fn(PullEvent) + Send + Sync)) -> Result<PullReport> {
    on_event(PullEvent::Checking);
    let build = client.latest_build(&opts.game, &opts.platform).await?;
    build.manifest.validate().context("server sent an invalid manifest")?;
    build.launch.validate(&build.manifest).context("server sent an invalid launch config")?;

    let dir = opts.dir.clone();
    tokio::fs::create_dir_all(&dir).await.with_context(|| format!("creating {}", dir.display()))?;
    let installed = blocking({
        let d = dir.clone();
        move || install::read_installed(&d)
    })
    .await?;
    if let Some(s) = &installed {
        if s.game != opts.game {
            bail!("{} already contains a different game ({})", dir.display(), s.game);
        }
    }
    let from_version = installed.as_ref().map(|s| s.version.clone());
    let new_state = InstalledState {
        game: opts.game.clone(),
        version: build.version.clone(),
        platform: opts.platform.clone(),
        launch: build.launch.clone(),
        installed_at: unix_now(),
        manifest: build.manifest.clone(),
    };

    let mut report = PullReport {
        game: opts.game.clone(),
        platform: opts.platform.clone(),
        from_version,
        to_version: build.version.clone(),
        changelog: build.changelog.clone(),
        up_to_date: false,
        downloaded_chunks: 0,
        downloaded_bytes: 0,
        reused_bytes: 0,
        files_written: 0,
        files_deleted: 0,
    };

    let same_files = installed
        .as_ref()
        .is_some_and(|s| s.platform == opts.platform && s.manifest.hash() == build.manifest_hash);
    if same_files && !opts.repair {
        // Files are identical; still record the version/launch settings in case only those changed.
        report.up_to_date = true;
        blocking({
            let d = dir.clone();
            move || install::write_installed(&d, &new_state)
        })
        .await?;
        return Ok(report);
    }

    let plan: Plan = blocking({
        let d = dir.clone();
        let new = build.manifest.clone();
        let old = installed.map(|s| s.manifest);
        let repair = opts.repair;
        move || {
            let old: Option<Manifest> = if repair {
                // Trust nothing: re-chunk what is actually on disk.
                let paths = new.files.iter().map(|f| f.path.as_str());
                let previous = old.iter().flat_map(|m| m.files.iter().map(|f| f.path.as_str()));
                Some(install::scan_existing(&d, paths.chain(previous))?)
            } else {
                old
            };
            install::plan(&d, old.as_ref(), &new)
        }
    })
    .await?;

    on_event(PullEvent::Planned {
        files_to_write: plan.write_files.len(),
        files_to_delete: plan.delete_files.len(),
        download_bytes: plan.download_bytes(),
        reused_bytes: plan.reused_bytes(),
    });

    let cache = chunk_cache_dir(&dir);
    tokio::fs::create_dir_all(&cache).await?;
    let total_chunks = plan.download.len();
    let total_bytes = plan.download_bytes();
    let mut downloads = futures::stream::iter(plan.download.iter().cloned())
        .map(|chunk| {
            let cache = cache.clone();
            async move {
                let dest = cache.join(&chunk.hash);
                // Resume: a chunk fetched by an interrupted earlier attempt is reused if intact.
                let cached = blocking({
                    let (dest, hash) = (dest.clone(), chunk.hash.clone());
                    move || Ok(std::fs::read(&dest).map(|d| hash_bytes(&d) == hash).unwrap_or(false))
                })
                .await?;
                if !cached {
                    let compressed = with_retries(|| client.download_chunk(&chunk.hash)).await?;
                    blocking({
                        let hash = chunk.hash.clone();
                        move || {
                            let data = decompress_verified(&hash, &compressed)?;
                            let tmp = dest.with_extension("part");
                            std::fs::write(&tmp, data)?;
                            std::fs::rename(&tmp, &dest)?;
                            Ok(())
                        }
                    })
                    .await?;
                }
                Ok::<u64, anyhow::Error>(chunk.size as u64)
            }
        })
        .buffer_unordered(DOWNLOAD_CONCURRENCY)
        .into_stream();
    let (mut done, mut done_bytes) = (0usize, 0u64);
    while let Some(size) = downloads.try_next().await? {
        done += 1;
        done_bytes += size;
        on_event(PullEvent::Downloading { done_chunks: done, total_chunks, done_bytes, total_bytes });
    }
    drop(downloads);

    on_event(PullEvent::Applying);
    report.downloaded_chunks = total_chunks;
    report.downloaded_bytes = total_bytes;
    report.reused_bytes = plan.reused_bytes();
    report.files_written = plan.write_files.len();
    report.files_deleted = plan.delete_files.len();
    blocking({
        let d = dir.clone();
        move || {
            install::apply(&d, &plan, &new_state.manifest)?;
            install::write_installed(&d, &new_state)?;
            let _ = std::fs::remove_dir_all(chunk_cache_dir(&d));
            Ok(())
        }
    })
    .await?;
    Ok(report)
}

/// Read what is installed in `dir`, if anything.
pub fn installed(dir: &Path) -> Result<Option<InstalledState>> {
    install::read_installed(dir)
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await?
}
