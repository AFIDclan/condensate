use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use bytes::Bytes;

use crate::error::{bad_request, ApiResult};

/// Content-addressed chunk store: `<data>/chunks/<first two hex chars>/<hash>.zst`.
pub struct Storage {
    chunks: PathBuf,
    images: PathBuf,
    tmp: PathBuf,
    counter: AtomicU64,
}

impl Storage {
    pub fn new(data_dir: &Path) -> Result<Self> {
        let chunks = data_dir.join("chunks");
        let images = data_dir.join("images");
        let tmp = data_dir.join("tmp");
        std::fs::create_dir_all(&chunks)?;
        std::fs::create_dir_all(&images)?;
        std::fs::create_dir_all(&tmp)?;
        std::fs::create_dir_all(data_dir.join("downloads"))?;
        Ok(Self { chunks, images, tmp, counter: AtomicU64::new(0) })
    }

    pub fn chunks_dir(&self) -> &Path {
        &self.chunks
    }

    pub fn images_dir(&self) -> &Path {
        &self.images
    }

    /// A unique temporary path on the same filesystem, for write-then-rename.
    pub fn tmp_path(&self, name: &str) -> PathBuf {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        self.tmp.join(format!("{name}.{n}"))
    }

    fn chunk_path(&self, hash: &str) -> PathBuf {
        self.chunks.join(&hash[..2]).join(format!("{hash}.zst"))
    }

    pub async fn has_chunk(&self, hash: &str) -> bool {
        tokio::fs::try_exists(self.chunk_path(hash)).await.unwrap_or(false)
    }

    /// Store a zstd-compressed chunk after checking it decompresses to data with `hash`.
    pub async fn put_chunk(&self, hash: &str, compressed: Bytes) -> ApiResult<()> {
        let hash = hash.to_string();
        let dest = self.chunk_path(&hash);
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        let tmp = self.tmp.join(format!("{hash}.{n}"));
        tokio::task::spawn_blocking(move || {
            condensate_core::chunk::decompress_verified(&hash, &compressed)
                .map_err(|e| bad_request(format!("rejected chunk: {e}")))?;
            let write = || -> Result<()> {
                std::fs::create_dir_all(dest.parent().unwrap())?;
                std::fs::write(&tmp, &compressed).context("writing chunk")?;
                std::fs::rename(&tmp, &dest).context("storing chunk")?;
                Ok(())
            };
            write().map_err(Into::into)
        })
        .await?
    }

    pub async fn read_chunk(&self, hash: &str) -> std::io::Result<Vec<u8>> {
        tokio::fs::read(self.chunk_path(hash)).await
    }
}
