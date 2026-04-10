use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use sha2::{Digest, Sha256};

/// Global cache epoch. Bump this single constant to invalidate ALL caches
/// (bytecode `.fxc`, module bytecode `.fxm`, module interfaces `.flxi`,
/// and native `.o` metadata) at once.
///
/// This replaces the need to coordinate 4 separate `FORMAT_VERSION` constants
/// across different cache modules. Each cache type embeds this epoch and
/// rejects entries written with a different value.
///
/// Epoch 1: initial unified epoch (replaces FXBC=11, FXMC=2, flxi=3, native=2).
/// Epoch 2: fix parse_int HM signature (String -> Int, was String -> Option<Int>).
/// Epoch 3: portable symbol table in .flxi (re-intern Symbols across sessions).
/// Epoch 4: relocatable module bytecode round-trips effect descriptors.
/// Epoch 5: generated class-dispatch functions are injected into module bodies
/// for cached VM assembly, preserving `Module.member` exports.
/// Epoch 6: cached module artifacts omit unreferenced imported globals, so
/// interface-only preloads do not become bogus linker dependencies.
/// Epoch 7: cached class dispatch splits module-member stubs from global
/// `__tc_*` instance functions, preserving both export conventions.
pub const CACHE_EPOCH: u16 = 7;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheLayout {
    root: PathBuf,
}

impl CacheLayout {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn interfaces_dir(&self) -> PathBuf {
        self.root.join("interfaces")
    }

    pub fn vm_dir(&self) -> PathBuf {
        self.root.join("vm")
    }

    pub fn native_dir(&self) -> PathBuf {
        self.root.join("native")
    }
}

pub fn resolve_cache_layout(entry_file: &Path, cache_dir: Option<&Path>) -> CacheLayout {
    CacheLayout {
        root: resolve_cache_root(entry_file, cache_dir),
    }
}

pub fn resolve_cache_root(entry_file: &Path, cache_dir: Option<&Path>) -> PathBuf {
    if let Some(dir) = cache_dir {
        return absolutize(dir);
    }

    if let Some(project_root) = find_project_root(entry_file) {
        return project_root.join("target").join("flux");
    }

    entry_directory(entry_file).join(".flux").join("cache")
}

pub fn find_project_root(entry_file: &Path) -> Option<PathBuf> {
    let mut current = absolutize(entry_file);
    if current.is_file() {
        current.pop();
    }

    loop {
        if current.join("Cargo.toml").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

pub fn interface_cache_path(cache_root: &Path, source_path: &Path) -> PathBuf {
    cache_root
        .join("interfaces")
        .join(format!("{}.flxi", artifact_stem(source_path)))
}

pub fn cache_key_filename(source_path: &Path, cache_key: &[u8; 32], ext: &str) -> String {
    format!(
        "{}-{}.{}",
        artifact_stem(source_path),
        hex_prefix(cache_key, 16),
        ext
    )
}

pub fn artifact_stem(source_path: &Path) -> String {
    let readable = source_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(sanitize_component)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "module".to_string());
    let path_hash = short_path_hash(source_path);
    format!("{readable}-{path_hash}")
}

fn entry_directory(entry_file: &Path) -> PathBuf {
    let absolute = absolutize(entry_file);
    if absolute.is_dir() {
        absolute
    } else {
        absolute
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }

    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path.to_path_buf(),
    }
}

fn short_path_hash(path: &Path) -> String {
    let canonicalish = fs::canonicalize(path).unwrap_or_else(|_| absolutize(path));
    let mut hasher = Sha256::new();
    hasher.update(normalize_for_hash(&canonicalish));
    let digest = hasher.finalize();
    hex_prefix(digest.as_slice(), 12)
}

fn normalize_for_hash(path: &Path) -> String {
    let mut normalized = String::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push_str(&prefix.as_os_str().to_string_lossy()),
            Component::RootDir => normalized.push('/'),
            Component::CurDir => normalized.push('.'),
            Component::ParentDir => normalized.push_str(".."),
            Component::Normal(part) => normalized.push_str(&part.to_string_lossy()),
        }
        normalized.push('/');
    }
    normalized
}

fn sanitize_component(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn hex_prefix(bytes: &[u8], len: usize) -> String {
    let mut out = String::with_capacity(len * 2);
    for b in bytes.iter().take(len) {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        artifact_stem, cache_key_filename, find_project_root, interface_cache_path,
        resolve_cache_root,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn finds_repo_root_from_nested_entry() {
        let entry = Path::new("examples/aoc/2024/day06.flx");
        let root = find_project_root(entry).expect("expected Cargo project root");
        assert!(root.ends_with("flux"));
    }

    #[test]
    fn resolves_repo_cache_root_to_target_flux() {
        let entry = Path::new("examples/aoc/2024/day06.flx");
        let root = resolve_cache_root(entry, None);
        assert!(root.ends_with(Path::new("flux/target/flux")));
    }

    #[test]
    fn resolves_non_cargo_cache_root_to_local_flux_cache() {
        let entry = PathBuf::from("/tmp/flux-standalone/example.flx");
        let root = resolve_cache_root(&entry, None);
        // On Windows, /tmp resolves to the current drive (e.g. E:/tmp).
        // Check the suffix instead of the full path.
        let suffix = Path::new("flux-standalone").join(".flux").join("cache");
        assert!(
            root.ends_with(&suffix),
            "expected root to end with {}, got {}",
            suffix.display(),
            root.display()
        );
    }

    #[test]
    fn explicit_cache_dir_wins() {
        let entry = Path::new("examples/aoc/2024/day06.flx");
        let root = resolve_cache_root(entry, Some(Path::new("tmp/cache")));
        assert!(root.ends_with(Path::new("tmp/cache")));
    }

    #[test]
    fn interface_paths_live_under_interfaces_dir() {
        let root = Path::new("/tmp/flux-cache");
        let path = interface_cache_path(root, Path::new("examples/aoc/2024/day06.flx"));
        assert_eq!(
            path.parent().unwrap(),
            Path::new("/tmp/flux-cache/interfaces")
        );
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".flxi")
        );
    }

    #[test]
    fn cache_filenames_include_path_hash_and_cache_key() {
        let filename =
            cache_key_filename(Path::new("examples/shared/Main.flx"), &[0xabu8; 32], "fxm");
        assert!(filename.starts_with("Main-"));
        assert!(filename.ends_with(".fxm"));
        assert!(filename.contains("-abababababababababababababababab."));
    }

    #[test]
    fn artifact_stem_changes_for_same_basename_in_different_dirs() {
        let a = artifact_stem(Path::new("examples/alpha/Main.flx"));
        let b = artifact_stem(Path::new("examples/beta/Main.flx"));
        assert_ne!(a, b);
    }
}
