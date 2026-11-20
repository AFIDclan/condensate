use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use condensate_core::api::{PublishBuild, PublishRequest};
use condensate_core::chunk::{compress, hash_bytes};
use condensate_core::scan::{read_at, scan_dir, ChunkLocation};
use condensate_core::{platform, LaunchConfig};
use futures::{StreamExt, TryStreamExt};
use serde::Serialize;

use crate::{with_retries, Client};

const UPLOAD_CONCURRENCY: usize = 6;

pub struct PushBuild {
    pub platform: String,
    pub dir: PathBuf,
    pub launch: LaunchConfig,
}

pub struct PushOptions {
    pub game: String,
    /// `None` lets the server bump the previous version.
    pub version: Option<String>,
    pub changelog: String,
    pub builds: Vec<PushBuild>,
}

#[derive(Debug, Clone)]
pub enum PushEvent {
    Scanning { platform: String },
    Scanned { platform: String, files: usize, bytes: u64, skipped: Vec<String> },
    Uploading { chunks: usize, bytes: u64 },
    Uploaded { chunks: usize, bytes: u64 },
    Publishing,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildStats {
    pub platform: String,
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PushReport {
    pub game: String,
    pub version: String,
    pub builds: Vec<BuildStats>,
    pub total_chunks: usize,
    pub uploaded_chunks: usize,
    /// Uncompressed size of the chunks that were new to the server.
    pub uploaded_bytes: u64,
}

/// Scan each build folder, upload only chunks the server lacks, then publish the version.
pub async fn push(client: &Client, opts: PushOptions, on_event: &(dyn Fn(PushEvent) + Send + Sync)) -> Result<PushReport> {
    if opts.changelog.trim().is_empty() {
        bail!("a changelog is required (one or two lines describing what changed)");
    }
    if opts.builds.is_empty() {
        bail!("nothing to push: no builds given");
    }

    let mut builds = Vec::new();
    let mut stats = Vec::new();
    let mut locations: HashMap<String, ChunkLocation> = HashMap::new();
    for b in opts.builds {
        if !platform::is_known(&b.platform) {
            bail!("unknown platform {:?}; use one of {:?}", b.platform, platform::ALL);
        }
        on_event(PushEvent::Scanning { platform: b.platform.clone() });
        let dir = b.dir.clone();
        let scan = tokio::task::spawn_blocking(move || scan_dir(&dir))
            .await?
            .with_context(|| format!("scanning {}", b.dir.display()))?;
        let mut launch = b.launch;
        launch.exe = normalize_rel(&launch.exe);
        launch.workdir = launch.workdir.map(|w| normalize_rel(&w));
        launch.validate(&scan.manifest).with_context(|| format!("{} build in {}", b.platform, b.dir.display()))?;

        let files = scan.manifest.files.len();
        let bytes = scan.manifest.total_size();
        on_event(PushEvent::Scanned { platform: b.platform.clone(), files, bytes, skipped: scan.skipped });
        stats.push(BuildStats { platform: b.platform.clone(), files, bytes });
        for (h, l) in scan.locations {
            locations.entry(h).or_insert(l);
        }
        builds.push(PublishBuild { platform: b.platform, manifest: scan.manifest, launch });
    }

    let all: Vec<String> = locations.keys().cloned().collect();
    let mut missing = Vec::new();
    for batch in all.chunks(5000) {
        missing.extend(client.missing_chunks(batch).await?);
    }
    let upload_bytes: u64 = missing.iter().map(|h| locations[h].size as u64).sum();
    on_event(PushEvent::Uploading { chunks: missing.len(), bytes: upload_bytes });

    let mut uploads = futures::stream::iter(missing.iter().cloned())
        .map(|hash| {
            let loc = locations[&hash].clone();
            async move {
                let size = loc.size as u64;
                let h = hash.clone();
                let compressed = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
                    let data = read_at(&loc.path, loc.offset, loc.size)?;
                    if hash_bytes(&data) != h {
                        bail!("{} changed while it was being pushed", loc.path.display());
                    }
                    compress(&data)
                })
                .await??;
                with_retries(|| client.upload_chunk(&hash, compressed.clone())).await?;
                Ok::<u64, anyhow::Error>(size)
            }
        })
        .buffer_unordered(UPLOAD_CONCURRENCY)
        .into_stream();
    let (mut done, mut done_bytes) = (0usize, 0u64);
    while let Some(size) = uploads.try_next().await? {
        done += 1;
        done_bytes += size;
        on_event(PushEvent::Uploaded { chunks: done, bytes: done_bytes });
    }
    drop(uploads);

    on_event(PushEvent::Publishing);
    let resp = client
        .publish(&opts.game, &PublishRequest { version: opts.version, changelog: opts.changelog.trim().into(), builds })
        .await?;

    Ok(PushReport {
        game: opts.game,
        version: resp.version,
        builds: stats,
        total_chunks: all.len(),
        uploaded_chunks: missing.len(),
        uploaded_bytes: upload_bytes,
    })
}

/// Accept `.\Game.exe` or `bin\game` style paths from Windows users.
fn normalize_rel(p: &str) -> String {
    let p = p.replace('\\', "/");
    let mut p = p.as_str();
    while let Some(rest) = p.strip_prefix("./") {
        p = rest;
    }
    p.to_string()
}
