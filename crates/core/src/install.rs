//! The install/update engine.
//!
//! 1. [`plan`] compares the installed manifest with the new one, finds which files must be
//!    (re)written and which of their chunks can be copied out of files already on disk.
//! 2. The caller downloads the remaining chunks into [`chunk_cache_dir`] (see the SDK).
//! 3. [`apply`] assembles changed files in a staging area, then swaps them into place.
//!
//! Old files are never modified until every new file is fully staged, so an interrupted update
//! leaves the previous version runnable and the next attempt resumes from the chunk cache.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::chunk::hash_bytes;
use crate::manifest::{native_path, validate_path, ChunkRef, FileEntry, LaunchConfig, Manifest};
use crate::scan::{chunk_file, read_at};
use crate::META_DIR;

/// Written to `<install>/.condensate/installed.json` after a successful install or update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledState {
    pub game: String,
    pub version: String,
    pub platform: String,
    pub launch: LaunchConfig,
    pub installed_at: i64,
    pub manifest: Manifest,
}

pub fn meta_dir(root: &Path) -> PathBuf {
    root.join(META_DIR)
}

pub fn chunk_cache_dir(root: &Path) -> PathBuf {
    meta_dir(root).join("chunks")
}

fn state_path(root: &Path) -> PathBuf {
    meta_dir(root).join("installed.json")
}

pub fn read_installed(root: &Path) -> Result<Option<InstalledState>> {
    let path = state_path(root);
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).with_context(|| format!("corrupt {}", path.display()))?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn write_installed(root: &Path, state: &InstalledState) -> Result<()> {
    fs::create_dir_all(meta_dir(root))?;
    let path = state_path(root);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct LocalChunk {
    pub path: PathBuf,
    pub offset: u64,
    pub size: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// Files that must be written (new or changed).
    pub write_files: Vec<FileEntry>,
    /// Files from the previous install that are no longer part of the game.
    pub delete_files: Vec<String>,
    /// Distinct chunks that must come from the server.
    pub download: Vec<ChunkRef>,
    /// Chunks that can be copied from files already on disk (verified while planning).
    pub local: HashMap<String, LocalChunk>,
    pub unchanged_files: usize,
}

impl Plan {
    pub fn download_bytes(&self) -> u64 {
        self.download.iter().map(|c| c.size as u64).sum()
    }

    pub fn reused_bytes(&self) -> u64 {
        self.local.values().map(|c| c.size as u64).sum()
    }

    pub fn is_noop(&self) -> bool {
        self.write_files.is_empty() && self.delete_files.is_empty()
    }
}

/// Work out how to turn the install at `root` (described by `old`) into `new`.
pub fn plan(root: &Path, old: Option<&Manifest>, new: &Manifest) -> Result<Plan> {
    new.validate()?;

    // Index the files of the old install that are still intact on disk.
    let mut old_files: HashMap<&str, &FileEntry> = HashMap::new();
    let mut candidates: HashMap<&str, LocalChunk> = HashMap::new();
    for f in old.iter().flat_map(|m| &m.files) {
        if validate_path(&f.path).is_err() {
            continue;
        }
        let abs = native_path(root, &f.path);
        let intact = fs::metadata(&abs).map(|m| m.is_file() && m.len() == f.size).unwrap_or(false);
        if !intact {
            continue;
        }
        old_files.insert(f.path.as_str(), f);
        let mut offset = 0u64;
        for c in &f.chunks {
            candidates
                .entry(c.hash.as_str())
                .or_insert_with(|| LocalChunk { path: abs.clone(), offset, size: c.size });
            offset += c.size as u64;
        }
    }

    let mut plan = Plan::default();
    for f in &new.files {
        match old_files.get(f.path.as_str()) {
            Some(of) if of.chunks == f.chunks => plan.unchanged_files += 1,
            _ => plan.write_files.push(f.clone()),
        }
    }

    let mut needed = HashSet::new();
    for c in plan.write_files.iter().flat_map(|f| &f.chunks) {
        if !needed.insert(c.hash.as_str()) {
            continue;
        }
        let reusable = candidates.get(c.hash.as_str()).filter(|l| {
            read_at(&l.path, l.offset, l.size).map(|d| hash_bytes(&d) == c.hash).unwrap_or(false)
        });
        match reusable {
            Some(l) => {
                plan.local.insert(c.hash.clone(), l.clone());
            }
            None => plan.download.push(c.clone()),
        }
    }

    if let Some(old) = old {
        let keep: HashSet<&str> = new.files.iter().map(|f| f.path.as_str()).collect();
        for f in &old.files {
            if !keep.contains(f.path.as_str()) && validate_path(&f.path).is_ok() {
                plan.delete_files.push(f.path.clone());
            }
        }
    }
    Ok(plan)
}

/// Re-chunk whatever is actually on disk at `paths`, for repairing an install whose recorded
/// state cannot be trusted. Content-defined chunking means intact data still produces the same
/// chunks, so only damaged regions get re-downloaded.
pub fn scan_existing<'a>(root: &Path, paths: impl IntoIterator<Item = &'a str>) -> Result<Manifest> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for p in paths {
        if !seen.insert(p) || validate_path(p).is_err() {
            continue;
        }
        let abs = native_path(root, p);
        if abs.is_file() {
            files.push(chunk_file(&abs, p.to_string())?.0);
        }
    }
    Ok(Manifest::new(files))
}

/// Execute a plan. Every chunk in `plan.download` must already be in [`chunk_cache_dir`].
pub fn apply(root: &Path, plan: &Plan, new: &Manifest) -> Result<()> {
    let cache = chunk_cache_dir(root);
    let staging = meta_dir(root).join("staging");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;

    // 1. Assemble every changed file in staging while the old files are still intact.
    for (i, f) in plan.write_files.iter().enumerate() {
        let tmp = staging.join(i.to_string());
        let mut out = BufWriter::new(fs::File::create(&tmp)?);
        for c in &f.chunks {
            let data = match plan.local.get(&c.hash) {
                Some(l) => read_at(&l.path, l.offset, l.size)?,
                None => fs::read(cache.join(&c.hash))
                    .with_context(|| format!("chunk {} was not downloaded", c.hash))?,
            };
            if hash_bytes(&data) != c.hash {
                bail!("chunk {} failed verification while writing {}", c.hash, f.path);
            }
            out.write_all(&data)?;
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    }

    // 2. Remove files that are no longer part of the game (never touches unknown files, so
    //    saves and settings the game wrote itself survive).
    for p in &plan.delete_files {
        let abs = native_path(root, p);
        if abs.is_file() {
            fs::remove_file(&abs).with_context(|| format!("removing {p} (is the game running?)"))?;
        }
        prune_empty_dirs(root, &abs);
    }

    // 3. Swap the new files into place.
    for (i, f) in plan.write_files.iter().enumerate() {
        let dest = native_path(root, &f.path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating folder for {}", f.path))?;
        }
        if dest.is_dir() {
            bail!("cannot replace folder {} with a file", f.path);
        }
        fs::rename(staging.join(i.to_string()), &dest)
            .with_context(|| format!("replacing {} (is the game running?)", f.path))?;
    }

    set_exec_bits(root, new)?;
    let _ = fs::remove_dir_all(&staging);
    Ok(())
}

fn prune_empty_dirs(root: &Path, removed: &Path) {
    let mut dir = removed.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

#[cfg(unix)]
pub fn set_exec_bits(root: &Path, manifest: &Manifest) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for f in manifest.files.iter().filter(|f| f.executable) {
        let p = native_path(root, &f.path);
        if let Ok(meta) = fs::metadata(&p) {
            let mut perm = meta.permissions();
            if perm.mode() & 0o111 != 0o111 {
                perm.set_mode(perm.mode() | 0o111);
                fs::set_permissions(&p, perm)?;
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn set_exec_bits(_root: &Path, _manifest: &Manifest) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::compress;
    use crate::scan::scan_dir;

    fn tempdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("condensate-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Deterministic pseudo-random bytes so chunk boundaries are realistic.
    fn noise(seed: u64, len: usize) -> Vec<u8> {
        let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    }

    /// Simulates the server: chunks downloaded from `src` into the install's cache.
    fn fetch(src: &HashMap<String, crate::scan::ChunkLocation>, dest: &Path, plan: &Plan) {
        let cache = chunk_cache_dir(dest);
        fs::create_dir_all(&cache).unwrap();
        for c in &plan.download {
            let l = &src[&c.hash];
            let data = read_at(&l.path, l.offset, l.size).unwrap();
            let packed = compress(&data).unwrap();
            let unpacked = crate::chunk::decompress_verified(&c.hash, &packed).unwrap();
            fs::write(cache.join(&c.hash), unpacked).unwrap();
        }
    }

    #[test]
    fn install_then_incremental_update() {
        let v1 = tempdir("v1");
        let v2 = tempdir("v2");
        let install = tempdir("install");

        // v1: a big pack file, a small file, and a file that will be removed.
        let mut pack = noise(1, 12 * 1024 * 1024);
        fs::write(v1.join("game.pck"), &pack).unwrap();
        fs::create_dir_all(v1.join("bin")).unwrap();
        fs::write(v1.join("bin/run.sh"), b"#!/bin/sh\necho hi\n").unwrap();
        fs::write(v1.join("old.txt"), b"bye").unwrap();
        let s1 = scan_dir(&v1).unwrap();

        let p1 = plan(&install, None, &s1.manifest).unwrap();
        assert_eq!(p1.write_files.len(), 3);
        fetch(&s1.locations, &install, &p1);
        apply(&install, &p1, &s1.manifest).unwrap();
        assert_eq!(fs::read(install.join("game.pck")).unwrap(), pack);

        // v2: patch a few bytes in the middle of the pack, drop old.txt, add new.txt.
        pack[6 * 1024 * 1024..6 * 1024 * 1024 + 100].copy_from_slice(&[7u8; 100]);
        fs::write(v2.join("game.pck"), &pack).unwrap();
        fs::create_dir_all(v2.join("bin")).unwrap();
        fs::write(v2.join("bin/run.sh"), b"#!/bin/sh\necho hi\n").unwrap();
        fs::write(v2.join("new.txt"), b"hello").unwrap();
        let s2 = scan_dir(&v2).unwrap();

        let p2 = plan(&install, Some(&s1.manifest), &s2.manifest).unwrap();
        assert_eq!(p2.unchanged_files, 1, "run.sh is unchanged");
        assert_eq!(p2.delete_files, vec!["old.txt".to_string()]);
        // Only the edited region of the 12 MiB pack (plus new.txt) should need downloading.
        assert!(p2.download_bytes() < 6 * 1024 * 1024, "downloaded {}", p2.download_bytes());
        assert!(p2.reused_bytes() > 4 * 1024 * 1024);

        fetch(&s2.locations, &install, &p2);
        apply(&install, &p2, &s2.manifest).unwrap();
        assert_eq!(fs::read(install.join("game.pck")).unwrap(), pack);
        assert!(!install.join("old.txt").exists());
        assert_eq!(fs::read(install.join("new.txt")).unwrap(), b"hello");

        // Corrupt a file and repair it.
        let mut bad = pack.clone();
        bad[100] ^= 0xff;
        fs::write(install.join("game.pck"), &bad).unwrap();
        let actual = scan_existing(&install, s2.manifest.files.iter().map(|f| f.path.as_str())).unwrap();
        let p3 = plan(&install, Some(&actual), &s2.manifest).unwrap();
        assert_eq!(p3.write_files.len(), 1);
        fetch(&s2.locations, &install, &p3);
        apply(&install, &p3, &s2.manifest).unwrap();
        assert_eq!(fs::read(install.join("game.pck")).unwrap(), pack);

        for d in [v1, v2, install] {
            let _ = fs::remove_dir_all(d);
        }
    }
}
