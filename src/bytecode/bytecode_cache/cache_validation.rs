use std::{
    fs,
    io::Read,
    path::Path,
};

use sha2::{Digest, Sha256};

use super::cache_serialization::{read_string, read_u16};

pub(super) fn validate_magic(reader: &mut std::fs::File, magic: &[u8; 4]) -> Option<()> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf).ok()?;
    if &buf == magic {
        Some(())
    } else {
        None
    }
}

pub(super) fn validate_format_version(
    reader: &mut std::fs::File,
    expected: u16,
) -> Option<u16> {
    let version = read_u16(reader)?;
    if version == expected {
        Some(version)
    } else {
        None
    }
}

pub(super) fn validate_cache_key(
    reader: &mut std::fs::File,
    expected: &[u8; 32],
) -> Option<[u8; 32]> {
    let mut cached_key = [0u8; 32];
    reader.read_exact(&mut cached_key).ok()?;
    if &cached_key == expected {
        Some(cached_key)
    } else {
        None
    }
}

pub(super) fn read_deps_and_validate(
    reader: &mut std::fs::File,
    deps_count: usize,
) -> Option<()> {
    for _ in 0..deps_count {
        let dep_path = read_string(reader)?;
        let mut dep_hash = [0u8; 32];
        reader.read_exact(&mut dep_hash).ok()?;
        if hash_file(Path::new(&dep_path)).ok()? != dep_hash {
            return None;
        }
    }
    Some(())
}

pub(super) fn read_deps_with_status(
    reader: &mut std::fs::File,
    deps_count: usize,
) -> Option<Vec<(String, [u8; 32], bool)>> {
    let mut deps = Vec::with_capacity(deps_count);
    for _ in 0..deps_count {
        let dep_path = read_string(reader)?;
        let mut dep_hash = [0u8; 32];
        reader.read_exact(&mut dep_hash).ok()?;
        let valid = hash_file(Path::new(&dep_path))
            .ok()
            .map(|current| current == dep_hash)
            .unwrap_or(false);
        deps.push((dep_path, dep_hash, valid));
    }
    Some(deps)
}

pub fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

pub fn hash_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let data = fs::read(path)?;
    Ok(hash_bytes(&data))
}

pub fn hash_cache_key(source_hash: &[u8; 32], roots_hash: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(source_hash);
    hasher.update(roots_hash);
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}
