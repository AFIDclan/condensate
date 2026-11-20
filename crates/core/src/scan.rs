use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use fastcdc::v2020::StreamCDC;
use walkdir::WalkDir;

use crate::chunk::{hash_bytes, AVG_CHUNK, MAX_CHUNK, MIN_CHUNK};
use crate::manifest::{ChunkRef, FileEntry, Manifest};
use crate::META_DIR;

/// Where the bytes of a chunk can be read from on local disk.
#[derive(Debug, Clone)]
pub struct ChunkLocation {
    pub path: PathBuf,
    pub offset: u64,
    pub size: u32,
}

pub struct ScanResult {
    pub manifest: Manifest,
    /// One readable location per distinct chunk hash.
    pub locations: HashMap<String, ChunkLocation>,
    /// Entries that are neither files nor directories (e.g. symlinks), which are not shipped.
    pub skipped: Vec<String>,
}

/// Chunk every file under `root` into a manifest.
pub fn scan_dir(root: &Path) -> Result<ScanResult> {
    if !root.is_dir() {
        bail!("{} is not a directory", root.display());
    }
    let mut files = Vec::new();
    let mut locations = HashMap::new();
    let mut skipped = Vec::new();

    let walker = WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            !(e.depth() == 1 && e.file_name().to_string_lossy().eq_ignore_ascii_case(META_DIR))
        });
    for entry in walker {
        let entry = entry?;
        if entry.depth() == 0 || entry.file_type().is_dir() {
            continue;
        }
        let rel = to_manifest_path(entry.path().strip_prefix(root)?)?;
        if !entry.file_type().is_file() {
            skipped.push(rel);
            continue;
        }
        let (file, locs) = chunk_file(entry.path(), rel)?;
        for (hash, loc) in locs {
            locations.entry(hash).or_insert(loc);
        }
        files.push(file);
    }

    let manifest = Manifest::new(files);
    manifest.validate()?;
    Ok(ScanResult { manifest, locations, skipped })
}

/// Convert a relative native path into a `/`-separated manifest path.
pub fn to_manifest_path(rel: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => parts.push(
                s.to_str()
                    .ok_or_else(|| anyhow!("file name is not valid UTF-8: {}", rel.display()))?
                    .to_string(),
            ),
            other => bail!("unexpected path component {other:?} in {}", rel.display()),
        }
    }
    Ok(parts.join("/"))
}

/// Split one file into content-defined chunks.
pub fn chunk_file(path: &Path, rel: String) -> Result<(FileEntry, Vec<(String, ChunkLocation)>)> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut executable = has_exec_bit(&file.metadata()?);
    let mut chunks = Vec::new();
    let mut locs = Vec::new();
    for (i, res) in StreamCDC::new(file, MIN_CHUNK, AVG_CHUNK, MAX_CHUNK).enumerate() {
        let c = res.map_err(|e| anyhow!("reading {}: {e:?}", path.display()))?;
        // Builds are often exported on Windows, which has no exec bit, so also sniff the content.
        if i == 0 && (c.data.starts_with(b"\x7fELF") || c.data.starts_with(b"#!")) {
            executable = true;
        }
        let hash = hash_bytes(&c.data);
        let size = c.length as u32;
        locs.push((hash.clone(), ChunkLocation { path: path.to_path_buf(), offset: c.offset, size }));
        chunks.push(ChunkRef { hash, size });
    }
    let size = chunks.iter().map(|c| c.size as u64).sum();
    Ok((FileEntry { path: rel, size, executable, chunks }, locs))
}

/// Read `size` bytes at `offset` from a file.
pub fn read_at(path: &Path, offset: u64, size: u32) -> std::io::Result<Vec<u8>> {
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; size as usize];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

#[cfg(unix)]
fn has_exec_bit(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn has_exec_bit(_meta: &std::fs::Metadata) -> bool {
    false
}
