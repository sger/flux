use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    cache_paths,
    types::module_interface::DependencyFingerprint,
};

pub const NATIVE_MODULE_CACHE_FORMAT_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeModuleArtifactMetadata {
    pub compiler_version: String,
    pub format_version: u16,
    pub cache_key: String,
    pub dependency_fingerprints: Vec<DependencyFingerprint>,
    pub optimize: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeModuleCacheLoadError {
    NotFound,
    InvalidMetadata,
    CompilerVersionMismatch,
    FormatVersionMismatch,
    CacheKeyMismatch,
    DependencyFingerprintMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeModuleArtifactInfo {
    pub object_path: PathBuf,
    pub metadata_path: PathBuf,
    pub metadata: NativeModuleArtifactMetadata,
    pub dependency_statuses: Vec<DependencyFingerprintStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyFingerprintStatus {
    pub module_name: String,
    pub source_path: String,
    pub expected_fingerprint: String,
    pub current_fingerprint: Option<String>,
    pub status: DependencyStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyStatus {
    Ok,
    Missing,
    Stale,
}

impl NativeModuleCacheLoadError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::NotFound => "not found",
            Self::InvalidMetadata => "invalid metadata",
            Self::CompilerVersionMismatch => "compiler version mismatch",
            Self::FormatVersionMismatch => "format version mismatch",
            Self::CacheKeyMismatch => "cache key mismatch",
            Self::DependencyFingerprintMismatch => "dependency fingerprint mismatch",
        }
    }
}

pub struct NativeModuleCache {
    cache_dir: PathBuf,
}

impl NativeModuleCache {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    pub fn object_path(&self, source_path: &Path, cache_key: &[u8; 32]) -> PathBuf {
        self.cache_dir
            .join(cache_paths::cache_key_filename(source_path, cache_key, object_ext()))
    }

    pub fn metadata_path(&self, source_path: &Path, cache_key: &[u8; 32]) -> PathBuf {
        self.cache_dir
            .join(cache_paths::cache_key_filename(source_path, cache_key, "fno"))
    }

    pub fn store(
        &self,
        source_path: &Path,
        cache_key: &[u8; 32],
        dependency_fingerprints: Vec<DependencyFingerprint>,
        optimize: bool,
    ) -> std::io::Result<PathBuf> {
        fs::create_dir_all(&self.cache_dir)?;
        let metadata = NativeModuleArtifactMetadata {
            compiler_version: env!("CARGO_PKG_VERSION").to_string(),
            format_version: NATIVE_MODULE_CACHE_FORMAT_VERSION,
            cache_key: hex::encode(cache_key),
            dependency_fingerprints,
            optimize,
        };
        let metadata_path = self.metadata_path(source_path, cache_key);
        fs::write(
            &metadata_path,
            serde_json::to_vec_pretty(&metadata).map_err(std::io::Error::other)?,
        )?;
        Ok(self.object_path(source_path, cache_key))
    }

    pub fn validate(
        &self,
        source_path: &Path,
        cache_key: &[u8; 32],
        cache_root: &Path,
    ) -> Result<PathBuf, NativeModuleCacheLoadError> {
        let metadata_path = self.metadata_path(source_path, cache_key);
        let object_path = self.object_path(source_path, cache_key);
        if !metadata_path.exists() || !object_path.exists() {
            return Err(NativeModuleCacheLoadError::NotFound);
        }
        let metadata: NativeModuleArtifactMetadata = serde_json::from_slice(
            &fs::read(&metadata_path).map_err(|_| NativeModuleCacheLoadError::InvalidMetadata)?,
        )
        .map_err(|_| NativeModuleCacheLoadError::InvalidMetadata)?;
        if metadata.compiler_version != env!("CARGO_PKG_VERSION") {
            return Err(NativeModuleCacheLoadError::CompilerVersionMismatch);
        }
        if metadata.format_version != NATIVE_MODULE_CACHE_FORMAT_VERSION {
            return Err(NativeModuleCacheLoadError::FormatVersionMismatch);
        }
        if metadata.cache_key != hex::encode(cache_key) {
            return Err(NativeModuleCacheLoadError::CacheKeyMismatch);
        }
        let dependency_statuses = self.inspect_dependency_statuses(&metadata, cache_root);
        if dependency_statuses
            .iter()
            .any(|status| status.status != DependencyStatus::Ok)
        {
            return Err(NativeModuleCacheLoadError::DependencyFingerprintMismatch);
        }
        Ok(object_path)
    }

    pub fn inspect(
        &self,
        source_path: &Path,
        cache_key: &[u8; 32],
        cache_root: &Path,
    ) -> Result<NativeModuleArtifactInfo, NativeModuleCacheLoadError> {
        let metadata_path = self.metadata_path(source_path, cache_key);
        let object_path = self.object_path(source_path, cache_key);
        if !metadata_path.exists() || !object_path.exists() {
            return Err(NativeModuleCacheLoadError::NotFound);
        }
        let metadata: NativeModuleArtifactMetadata = serde_json::from_slice(
            &fs::read(&metadata_path).map_err(|_| NativeModuleCacheLoadError::InvalidMetadata)?,
        )
        .map_err(|_| NativeModuleCacheLoadError::InvalidMetadata)?;
        let dependency_statuses = self.inspect_dependency_statuses(&metadata, cache_root);
        Ok(NativeModuleArtifactInfo {
            object_path,
            metadata_path,
            metadata,
            dependency_statuses,
        })
    }

    fn inspect_dependency_statuses(
        &self,
        metadata: &NativeModuleArtifactMetadata,
        cache_root: &Path,
    ) -> Vec<DependencyFingerprintStatus> {
        metadata
            .dependency_fingerprints
            .iter()
            .map(|dependency| {
                let dependency_path = PathBuf::from(&dependency.source_path);
                match crate::bytecode::compiler::module_interface::load_cached_interface(
                    cache_root,
                    &dependency_path,
                ) {
                    Ok(current) if current.interface_fingerprint == dependency.interface_fingerprint => {
                        DependencyFingerprintStatus {
                            module_name: dependency.module_name.clone(),
                            source_path: dependency.source_path.clone(),
                            expected_fingerprint: dependency.interface_fingerprint.clone(),
                            current_fingerprint: Some(current.interface_fingerprint),
                            status: DependencyStatus::Ok,
                        }
                    }
                    Ok(current) => DependencyFingerprintStatus {
                        module_name: dependency.module_name.clone(),
                        source_path: dependency.source_path.clone(),
                        expected_fingerprint: dependency.interface_fingerprint.clone(),
                        current_fingerprint: Some(current.interface_fingerprint),
                        status: DependencyStatus::Stale,
                    },
                    Err(_) => DependencyFingerprintStatus {
                        module_name: dependency.module_name.clone(),
                        source_path: dependency.source_path.clone(),
                        expected_fingerprint: dependency.interface_fingerprint.clone(),
                        current_fingerprint: None,
                        status: DependencyStatus::Missing,
                    },
                }
            })
            .collect()
    }
}

fn object_ext() -> &'static str {
    if cfg!(windows) { "obj" } else { "o" }
}

pub fn support_object_path(cache_layout: &cache_paths::CacheLayout, optimize: bool) -> PathBuf {
    cache_layout.native_dir().join(if optimize {
        if cfg!(windows) {
            "flux_support_O2.obj"
        } else {
            "flux_support_O2.o"
        }
    } else if cfg!(windows) {
        "flux_support_O0.obj"
    } else {
        "flux_support_O0.o"
    })
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::bytecode_cache::hash_bytes;
    use crate::types::module_interface::{DependencyFingerprint, ModuleInterface};

    #[test]
    fn native_module_cache_roundtrips_metadata_validation() {
        let temp = std::env::temp_dir().join(format!(
            "flux-native-cache-{}",
            std::process::id()
        ));
        let cache = NativeModuleCache::new(temp.join("native"));
        let cache_root = temp.join("root");
        fs::create_dir_all(cache_root.join("interfaces")).expect("cache root");
        let source = Path::new("examples/aoc/2024/Day06Solver.flx");
        let dep_path = Path::new("lib/Flow/List.flx");
        let dep_interface_path = crate::bytecode::compiler::module_interface::interface_path(
            &cache_root,
            dep_path,
        );
        let mut dep_interface = ModuleInterface::new("Flow.List", "deadbeef", "config");
        dep_interface.interface_fingerprint = "feedface".to_string();
        crate::bytecode::compiler::module_interface::save_interface(&dep_interface_path, &dep_interface)
            .expect("save interface");
        let cache_key = hash_bytes(b"native");
        let object_path = cache
            .store(
                source,
                &cache_key,
                vec![DependencyFingerprint {
                    module_name: "Flow.List".to_string(),
                    source_path: dep_path.to_string_lossy().to_string(),
                    interface_fingerprint: dep_interface.interface_fingerprint.clone(),
                }],
                false,
            )
            .expect("store metadata");
        fs::write(&object_path, []).expect("write object");

        let validated = cache
            .validate(source, &cache_key, &cache_root)
            .expect("validate artifact");
        assert_eq!(validated, object_path);
    }

    #[test]
    fn native_module_cache_inspect_reports_stale_dependency() {
        let temp = std::env::temp_dir().join(format!(
            "flux-native-cache-stale-{}",
            std::process::id()
        ));
        let cache = NativeModuleCache::new(temp.join("native"));
        let cache_root = temp.join("root");
        fs::create_dir_all(cache_root.join("interfaces")).expect("cache root");
        let source = Path::new("examples/aoc/2024/Day06Solver.flx");
        let dep_path = Path::new("lib/Flow/List.flx");
        let dep_interface_path =
            crate::bytecode::compiler::module_interface::interface_path(&cache_root, dep_path);
        let mut dep_interface = ModuleInterface::new("Flow.List", "deadbeef", "config");
        dep_interface.interface_fingerprint = "feedface".to_string();
        crate::bytecode::compiler::module_interface::save_interface(&dep_interface_path, &dep_interface)
            .expect("save interface");
        let cache_key = hash_bytes(b"native-stale");
        let object_path = cache
            .store(
                source,
                &cache_key,
                vec![DependencyFingerprint {
                    module_name: "Flow.List".to_string(),
                    source_path: dep_path.to_string_lossy().to_string(),
                    interface_fingerprint: "expected".to_string(),
                }],
                false,
            )
            .expect("store metadata");
        fs::write(&object_path, []).expect("write object");

        let info = cache
            .inspect(source, &cache_key, &cache_root)
            .expect("inspect artifact");
        assert_eq!(info.dependency_statuses.len(), 1);
        assert_eq!(info.dependency_statuses[0].status, DependencyStatus::Stale);
        assert_eq!(
            info.dependency_statuses[0].current_fingerprint.as_deref(),
            Some("feedface")
        );
    }
}
