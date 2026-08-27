//! Content-addressed storage for compiled module artifacts.
//!
//! The compiler's ordinary cache remains project-local because its interface
//! records contain paths used for dependency validation. This store is the
//! reusable layer: an artifact is copied in after a successful build and can
//! be hydrated into a project cache on a later build. Store keys contain only
//! stable inputs (source bytes, a package-relative module name, compiler/ABI,
//! backend, and semantic inputs); paths and mtimes never enter the digest.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};

use crate::{
    driver::backend::Backend,
    shared::{cache_paths, hex},
};

/// The ABI namespace is separate from the user-visible compiler version.
pub(crate) const COMPILER_ABI: &str = "flux-fxmc-26";

pub(crate) fn store_root() -> PathBuf {
    let home = std::env::var_os("FLUX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|p| p.join(".flux")))
        .unwrap_or_else(|| PathBuf::from(".flux"));
    home.join("store")
}

fn backend_name(backend: Backend) -> &'static str {
    match backend {
        Backend::Vm => "vm",
        Backend::Native => "native",
    }
}

/// Compute a stable module-unit hash. `deps` should contain interface
/// fingerprints for direct imports; sorting makes the digest independent of
/// traversal order.
pub(crate) fn unit_hash(
    source_path: &Path,
    source: &str,
    compiler_cache_key: &[u8; 32],
    backend: Backend,
    deps: &[(String, String)],
) -> String {
    let (package, relative) = package_identity(source_path);
    let mut hasher = Sha256::new();
    feed(&mut hasher, "format", &cache_paths::CACHE_EPOCH.to_string());
    feed(&mut hasher, "compiler-abi", COMPILER_ABI);
    feed(&mut hasher, "compiler-version", env!("CARGO_PKG_VERSION"));
    feed(&mut hasher, "backend", backend_name(backend));
    feed(&mut hasher, "package", &package);
    feed(&mut hasher, "relative-source", &relative);
    hasher.update(b"source\0");
    hasher.update(source.as_bytes());
    hasher.update(b"cache-key\0");
    hasher.update(compiler_cache_key);
    let mut sorted = deps.to_vec();
    sorted.sort();
    for (path, fingerprint) in sorted {
        // The dependency path is only a label within the package graph. It is
        // normalized to the basename-relative identity before hashing.
        feed(&mut hasher, "dependency", &stable_dependency_label(&path));
        feed(&mut hasher, "fingerprint", &fingerprint);
    }
    hex::encode(&hasher.finalize())
}

fn feed(hasher: &mut Sha256, label: &str, value: &str) {
    hasher.update(label.as_bytes());
    hasher.update([0]);
    hasher.update(value.as_bytes());
    hasher.update([0]);
}

fn stable_dependency_label(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

fn package_identity(path: &Path) -> (String, String) {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut current = absolute.parent().map(Path::to_path_buf);
    while let Some(dir) = current {
        let manifest = dir.join("flux.toml");
        if manifest.is_file() {
            let text = fs::read_to_string(&manifest).unwrap_or_default();
            let package = manifest_string(&text, "name").unwrap_or_else(|| "package".into());
            let relative = absolute
                .strip_prefix(&dir)
                .ok()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|| {
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into()
                });
            return (package, relative);
        }
        current = dir.parent().map(Path::to_path_buf);
    }
    (
        "package".into(),
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into(),
    )
}

fn manifest_string(text: &str, key: &str) -> Option<String> {
    let in_package = text.lines().scan(false, |inside, line| {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            *inside = trimmed == "[package]";
        }
        Some((*inside, trimmed.to_string()))
    });
    for (inside, line) in in_package {
        if inside
            && let Some((left, right)) = line.split_once('=')
            && left.trim() == key
        {
            let value = right.trim().trim_matches('"');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn artifact_path(hash: &str, backend: Backend) -> PathBuf {
    store_root()
        .join(COMPILER_ABI)
        .join(hash)
        .join(backend_name(backend))
        .join("module.fxm")
}

/// Hydrate a project-local cache entry from an immutable store entry.
pub(crate) fn hydrate(local: &Path, hash: &str, backend: Backend) -> bool {
    let source = artifact_path(hash, backend);
    if !source.is_file() || local.is_file() {
        return false;
    }
    if let Some(parent) = local.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::copy(source, local).is_ok()
}

/// Publish a local artifact into the store. The final rename is deliberately
/// no-replace: if another process won, its artifact is adopted and ours is
/// discarded. This also avoids assuming builds are bit-for-bit reproducible.
pub(crate) fn publish(local: &Path, hash: &str, backend: Backend, reason: &str) {
    if !local.is_file() {
        return;
    }
    let final_file = artifact_path(hash, backend);
    if final_file.is_file() {
        return;
    }
    let incoming = store_root().join(".incoming").join(format!(
        "{}-{}-{}",
        std::process::id(),
        now_nanos(),
        hash
    ));
    let incoming_file = incoming.join("module.fxm");
    if fs::create_dir_all(incoming_file.parent().unwrap()).is_err()
        || fs::copy(local, &incoming_file).is_err()
    {
        let _ = fs::remove_dir_all(&incoming);
        return;
    }
    let _ = fs::write(incoming.join("reason.txt"), reason);
    let hash_parent = final_file.parent().and_then(Path::parent);
    if let Some(parent) = hash_parent {
        let _ = fs::create_dir_all(parent);
    }
    if fs::rename(&incoming, final_file.parent().unwrap()).is_err() {
        // The destination may have appeared between the initial check and
        // rename. The incoming directory is ours, so removing it is safe.
        let _ = fs::remove_dir_all(incoming);
    }
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::unit_hash;
    use crate::driver::backend::Backend;
    use std::path::Path;

    #[test]
    fn unit_hash_ignores_absolute_project_prefix() {
        let key = [7u8; 32];
        let a = unit_hash(
            Path::new("/one/src/Lib.flx"),
            "same",
            &key,
            Backend::Vm,
            &[],
        );
        let b = unit_hash(
            Path::new("/two/src/Lib.flx"),
            "same",
            &key,
            Backend::Vm,
            &[],
        );
        assert_eq!(a, b);
    }

    #[test]
    fn unit_hash_changes_for_backend_and_compiler_key() {
        let path = Path::new("src/Lib.flx");
        let a = unit_hash(path, "same", &[1u8; 32], Backend::Vm, &[]);
        let b = unit_hash(path, "same", &[2u8; 32], Backend::Vm, &[]);
        let c = unit_hash(path, "same", &[1u8; 32], Backend::Native, &[]);
        assert_ne!(a, b);
        assert_ne!(a, c);
    }
}
