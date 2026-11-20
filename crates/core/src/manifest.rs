use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::chunk::{is_valid_hash, MAX_CHUNK};
use crate::META_DIR;

pub const MANIFEST_FORMAT: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkRef {
    pub hash: String,
    pub size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Relative path using `/` separators.
    pub path: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub executable: bool,
    /// Chunks in file order; their sizes add up to `size`.
    pub chunks: Vec<ChunkRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub files: Vec<FileEntry>,
}

/// How to start a game once installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchConfig {
    /// Path of the executable relative to the install root, `/` separated.
    pub exe: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Working directory relative to the install root. Defaults to the executable's directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
}

impl Manifest {
    pub fn new(mut files: Vec<FileEntry>) -> Self {
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Self { format: MANIFEST_FORMAT, files }
    }

    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Stable identity of this exact file layout.
    pub fn hash(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("manifest always serializes");
        blake3::hash(&bytes).to_hex().to_string()
    }

    pub fn file(&self, path: &str) -> Option<&FileEntry> {
        self.files.iter().find(|f| f.path == path)
    }

    /// Every distinct chunk, in first-use order.
    pub fn unique_chunks(&self) -> Vec<ChunkRef> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for c in self.files.iter().flat_map(|f| &f.chunks) {
            if seen.insert(c.hash.as_str()) {
                out.push(c.clone());
            }
        }
        out
    }

    /// Reject anything that could escape the install directory or that would not install
    /// cleanly on Windows. Clients must call this before writing anything from a manifest.
    pub fn validate(&self) -> Result<()> {
        if self.format != MANIFEST_FORMAT {
            bail!("unsupported manifest format {}", self.format);
        }
        let mut seen = HashSet::new();
        for f in &self.files {
            validate_path(&f.path)?;
            if !seen.insert(f.path.to_lowercase()) {
                bail!("duplicate path (paths must differ by more than case): {}", f.path);
            }
            let mut sum = 0u64;
            for c in &f.chunks {
                if !is_valid_hash(&c.hash) {
                    bail!("invalid chunk hash in {}", f.path);
                }
                if c.size == 0 || c.size > MAX_CHUNK {
                    bail!("invalid chunk size in {}", f.path);
                }
                sum += c.size as u64;
            }
            if sum != f.size {
                bail!("chunk sizes do not add up to the file size of {}", f.path);
            }
        }
        // A path cannot be both a file and a directory.
        for f in &self.files {
            let lower = f.path.to_lowercase();
            for (i, ch) in lower.char_indices() {
                if ch == '/' && seen.contains(&lower[..i]) {
                    bail!("{} is both a file and a directory", &lower[..i]);
                }
            }
        }
        Ok(())
    }
}

impl LaunchConfig {
    pub fn validate(&self, manifest: &Manifest) -> Result<()> {
        validate_path(&self.exe)?;
        if manifest.file(&self.exe).is_none() {
            bail!("launch executable {:?} is not part of the build", self.exe);
        }
        if let Some(w) = &self.workdir {
            if w != "." {
                validate_path(w)?;
            }
        }
        Ok(())
    }
}

/// Validate a manifest-relative path (`/` separated, no `..`, nothing Windows can't store).
pub fn validate_path(path: &str) -> Result<()> {
    if path.is_empty() || path.len() > 1024 {
        bail!("invalid path length: {path:?}");
    }
    if path.starts_with('/') {
        bail!("path must be relative: {path:?}");
    }
    if path
        .chars()
        .any(|c| c.is_control() || matches!(c, '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*'))
    {
        bail!("path contains a character that is not allowed: {path:?}");
    }
    for (i, comp) in path.split('/').enumerate() {
        if comp.is_empty() || comp == "." || comp == ".." {
            bail!("invalid path component in {path:?}");
        }
        if comp.ends_with(' ') || comp.ends_with('.') {
            bail!("path components may not end with a space or dot: {path:?}");
        }
        if i == 0 && comp.eq_ignore_ascii_case(META_DIR) {
            bail!("{META_DIR} is reserved: {path:?}");
        }
    }
    Ok(())
}

/// Turn a validated manifest path into a native path under `root`.
pub fn native_path(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for part in rel.split('/') {
        p.push(part);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escapes() {
        for bad in ["../x", "a/../b", "/abs", "a\\b", "C:/x", "a//b", ".condensate/x", "a/b.", ""] {
            assert!(validate_path(bad).is_err(), "{bad} should be rejected");
        }
        for good in ["game.exe", "data/level 1.pck", ".hidden/file"] {
            assert!(validate_path(good).is_ok(), "{good} should be accepted");
        }
    }

    #[test]
    fn rejects_file_dir_conflict() {
        let file = |p: &str| FileEntry { path: p.into(), size: 0, executable: false, chunks: vec![] };
        assert!(Manifest::new(vec![file("a"), file("a/b")]).validate().is_err());
        assert!(Manifest::new(vec![file("a"), file("A")]).validate().is_err());
        assert!(Manifest::new(vec![file("a/c"), file("a/b")]).validate().is_ok());
    }
}
