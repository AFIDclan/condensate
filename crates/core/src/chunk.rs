use std::io::Read;

use anyhow::{bail, Context, Result};

/// Content-defined chunking parameters. Changing these changes every chunk boundary, which would
/// make every existing install re-download everything, so treat them as frozen.
pub const MIN_CHUNK: u32 = 256 * 1024;
pub const AVG_CHUNK: u32 = 1024 * 1024;
pub const MAX_CHUNK: u32 = 4 * 1024 * 1024;

const ZSTD_LEVEL: i32 = 3;

pub fn hash_bytes(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

pub fn is_valid_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn compress(data: &[u8]) -> Result<Vec<u8>> {
    Ok(zstd::bulk::compress(data, ZSTD_LEVEL)?)
}

/// Decompress a chunk as stored/transferred and check it really is the chunk `expected_hash`.
/// Refuses to inflate past [`MAX_CHUNK`], so a malicious upload cannot be a zip bomb.
pub fn decompress_verified(expected_hash: &str, compressed: &[u8]) -> Result<Vec<u8>> {
    let decoder = zstd::stream::read::Decoder::new(compressed).context("invalid zstd data")?;
    let mut out = Vec::new();
    decoder
        .take(MAX_CHUNK as u64 + 1)
        .read_to_end(&mut out)
        .context("could not decompress chunk")?;
    if out.len() > MAX_CHUNK as usize {
        bail!("chunk is larger than the {MAX_CHUNK} byte maximum");
    }
    let actual = hash_bytes(&out);
    if actual != expected_hash {
        bail!("chunk hash mismatch: expected {expected_hash}, got {actual}");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let data = b"hello condensate".repeat(1000);
        let hash = hash_bytes(&data);
        let packed = compress(&data).unwrap();
        assert_eq!(decompress_verified(&hash, &packed).unwrap(), data);
        assert!(decompress_verified(&hash_bytes(b"other"), &packed).is_err());
    }
}
