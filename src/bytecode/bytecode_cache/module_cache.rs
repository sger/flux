use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use crate::{
    bytecode::compiler::module_interface,
    bytecode::{
        bytecode_cache::cache_serialization::{
            read_function_debug_info, read_object, read_string, read_u32,
            write_function_debug_info, write_object, write_string, write_u16, write_u32,
        },
        bytecode_cache::cache_validation::{
            validate_cache_key, validate_format_version, validate_magic,
        },
        debug_info::FunctionDebugInfo,
    },
    cache_paths,
    diagnostics::position::Span,
    runtime::value::Value,
    types::module_interface::DependencyMissReason,
};

const MAGIC: &[u8; 4] = b"FXMC";
const FORMAT_VERSION: u16 = crate::cache_paths::CACHE_EPOCH;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleCacheLoadError {
    NotFound,
    BadMagic,
    FormatVersionMismatch,
    CompilerVersionMismatch,
    CacheKeyMismatch,
    DependencyFingerprintMismatch {
        path: String,
        reason: DependencyMissReason,
    },
    CorruptPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleCacheInspectInfo {
    pub cache_path: PathBuf,
    pub compiler_version: String,
    pub format_version: u16,
    pub cache_key: String,
    pub dependency_statuses: Vec<ModuleCacheDependencyStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleCacheDependencyStatus {
    pub source_path: String,
    pub expected_fingerprint: String,
    pub current_fingerprint: Option<String>,
    pub status: ModuleDependencyStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleDependencyStatus {
    Ok,
    Missing,
    Stale,
}

impl ModuleCacheLoadError {
    pub fn message(&self) -> String {
        match self {
            Self::NotFound => "not found".to_string(),
            Self::BadMagic => "bad magic".to_string(),
            Self::FormatVersionMismatch => "format version mismatch".to_string(),
            Self::CompilerVersionMismatch => "compiler version mismatch".to_string(),
            Self::CacheKeyMismatch => "cache key mismatch".to_string(),
            Self::DependencyFingerprintMismatch { path, reason } => {
                format!("dependency mismatch ({path}): {}", reason.label())
            }
            Self::CorruptPayload => "corrupt payload".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CachedModuleBinding {
    pub name: String,
    pub index: usize,
    pub span: Span,
    pub is_assigned: bool,
    pub kind: CachedModuleBindingKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachedModuleBindingKind {
    Defined = 0,
    Imported = 1,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CachedModuleBytecode {
    pub globals: Vec<CachedModuleBinding>,
    pub constants: Vec<Value>,
    pub instructions: Vec<u8>,
    pub debug_info: FunctionDebugInfo,
}

pub struct ModuleBytecodeCache {
    dir: PathBuf,
}

impl ModuleBytecodeCache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn load(
        &self,
        source_path: &Path,
        cache_key: &[u8; 32],
        compiler_version: &str,
        cache_root: &Path,
    ) -> Option<CachedModuleBytecode> {
        self.load_with_reason(source_path, cache_key, compiler_version, cache_root)
            .ok()
    }

    pub fn load_failure_reason(
        &self,
        source_path: &Path,
        cache_key: &[u8; 32],
        compiler_version: &str,
        cache_root: &Path,
    ) -> Option<String> {
        self.load_with_reason(source_path, cache_key, compiler_version, cache_root)
            .err()
            .map(|err| err.message())
    }

    fn load_with_reason(
        &self,
        source_path: &Path,
        cache_key: &[u8; 32],
        compiler_version: &str,
        cache_root: &Path,
    ) -> Result<CachedModuleBytecode, ModuleCacheLoadError> {
        let path = self.cache_path(source_path, cache_key);
        let mut file = File::open(path).map_err(|_| ModuleCacheLoadError::NotFound)?;

        validate_magic(&mut file, MAGIC).ok_or(ModuleCacheLoadError::BadMagic)?;
        validate_format_version(&mut file, FORMAT_VERSION)
            .ok_or(ModuleCacheLoadError::FormatVersionMismatch)?;

        let cached_compiler_version =
            read_string(&mut file).ok_or(ModuleCacheLoadError::CorruptPayload)?;
        if cached_compiler_version != compiler_version {
            return Err(ModuleCacheLoadError::CompilerVersionMismatch);
        }

        validate_cache_key(&mut file, cache_key).ok_or(ModuleCacheLoadError::CacheKeyMismatch)?;

        let deps_count = read_u32(&mut file).ok_or(ModuleCacheLoadError::CorruptPayload)? as usize;
        read_interface_deps_and_validate(&mut file, deps_count, cache_root)?;

        let globals_count =
            read_u32(&mut file).ok_or(ModuleCacheLoadError::CorruptPayload)? as usize;
        let mut globals = Vec::with_capacity(globals_count);
        for _ in 0..globals_count {
            globals.push(read_binding(&mut file).ok_or(ModuleCacheLoadError::CorruptPayload)?);
        }

        let constants_count =
            read_u32(&mut file).ok_or(ModuleCacheLoadError::CorruptPayload)? as usize;
        let mut constants = Vec::with_capacity(constants_count);
        for _ in 0..constants_count {
            constants.push(read_object(&mut file).ok_or(ModuleCacheLoadError::CorruptPayload)?);
        }

        let instructions_len =
            read_u32(&mut file).ok_or(ModuleCacheLoadError::CorruptPayload)? as usize;
        let mut instructions = vec![0u8; instructions_len];
        file.read_exact(&mut instructions)
            .map_err(|_| ModuleCacheLoadError::CorruptPayload)?;

        let debug_info =
            read_function_debug_info(&mut file).ok_or(ModuleCacheLoadError::CorruptPayload)?;

        Ok(CachedModuleBytecode {
            globals,
            constants,
            instructions,
            debug_info,
        })
    }

    pub fn store(
        &self,
        source_path: &Path,
        cache_key: &[u8; 32],
        compiler_version: &str,
        bytecode: &CachedModuleBytecode,
        deps: &[(String, String)],
    ) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.cache_path(source_path, cache_key);
        let mut file = File::create(path)?;

        file.write_all(MAGIC)?;
        write_u16(&mut file, FORMAT_VERSION)?;
        write_string(&mut file, compiler_version)?;
        file.write_all(cache_key)?;

        write_u32(&mut file, deps.len() as u32)?;
        for (dep_path, dep_fingerprint) in deps {
            write_string(&mut file, dep_path)?;
            write_string(&mut file, dep_fingerprint)?;
        }

        write_u32(&mut file, bytecode.globals.len() as u32)?;
        for binding in &bytecode.globals {
            write_binding(&mut file, binding)?;
        }

        write_u32(&mut file, bytecode.constants.len() as u32)?;
        for constant in &bytecode.constants {
            write_object(&mut file, constant)?;
        }

        write_u32(&mut file, bytecode.instructions.len() as u32)?;
        file.write_all(&bytecode.instructions)?;
        write_function_debug_info(&mut file, Some(&bytecode.debug_info))?;

        Ok(())
    }

    pub fn cache_path(&self, source_path: &Path, cache_key: &[u8; 32]) -> PathBuf {
        self.dir.join(cache_paths::cache_key_filename(
            source_path,
            cache_key,
            "fxm",
        ))
    }

    pub fn inspect(
        &self,
        source_path: &Path,
        cache_key: &[u8; 32],
        compiler_version: &str,
        cache_root: &Path,
    ) -> Result<ModuleCacheInspectInfo, ModuleCacheLoadError> {
        let path = self.cache_path(source_path, cache_key);
        let mut file = File::open(&path).map_err(|_| ModuleCacheLoadError::NotFound)?;

        validate_magic(&mut file, MAGIC).ok_or(ModuleCacheLoadError::BadMagic)?;
        validate_format_version(&mut file, FORMAT_VERSION)
            .ok_or(ModuleCacheLoadError::FormatVersionMismatch)?;

        let cached_compiler_version =
            read_string(&mut file).ok_or(ModuleCacheLoadError::CorruptPayload)?;
        if cached_compiler_version != compiler_version {
            return Err(ModuleCacheLoadError::CompilerVersionMismatch);
        }

        validate_cache_key(&mut file, cache_key).ok_or(ModuleCacheLoadError::CacheKeyMismatch)?;

        let deps_count = read_u32(&mut file).ok_or(ModuleCacheLoadError::CorruptPayload)? as usize;
        let dependency_statuses = read_interface_deps(reader_ref(&mut file), deps_count)?
            .into_iter()
            .map(|(dep_path, expected_fingerprint)| {
                let dep_source_path = Path::new(&dep_path);
                match module_interface::load_cached_interface(cache_root, dep_source_path) {
                    Ok(interface) if interface.interface_fingerprint == expected_fingerprint => {
                        ModuleCacheDependencyStatus {
                            source_path: dep_path,
                            expected_fingerprint,
                            current_fingerprint: Some(interface.interface_fingerprint),
                            status: ModuleDependencyStatus::Ok,
                        }
                    }
                    Ok(interface) => ModuleCacheDependencyStatus {
                        source_path: dep_path,
                        expected_fingerprint,
                        current_fingerprint: Some(interface.interface_fingerprint),
                        status: ModuleDependencyStatus::Stale,
                    },
                    Err(_) => ModuleCacheDependencyStatus {
                        source_path: dep_path,
                        expected_fingerprint,
                        current_fingerprint: None,
                        status: ModuleDependencyStatus::Missing,
                    },
                }
            })
            .collect();

        Ok(ModuleCacheInspectInfo {
            cache_path: path,
            compiler_version: cached_compiler_version,
            format_version: FORMAT_VERSION,
            cache_key: hex::encode(cache_key),
            dependency_statuses,
        })
    }
}

fn read_interface_deps_and_validate(
    reader: &mut File,
    deps_count: usize,
    cache_root: &Path,
) -> Result<(), ModuleCacheLoadError> {
    for _ in 0..deps_count {
        let dep_path = read_string(reader).ok_or(ModuleCacheLoadError::CorruptPayload)?;
        let expected_fingerprint =
            read_string(reader).ok_or(ModuleCacheLoadError::CorruptPayload)?;
        let dep_source_path = Path::new(&dep_path);
        let interface = module_interface::load_cached_interface(cache_root, dep_source_path)
            .map_err(|_| ModuleCacheLoadError::DependencyFingerprintMismatch {
                path: dep_path.clone(),
                reason: DependencyMissReason::InterfaceMissing,
            })?;
        if interface.interface_fingerprint != expected_fingerprint {
            return Err(ModuleCacheLoadError::DependencyFingerprintMismatch {
                path: dep_path,
                reason: DependencyMissReason::InterfaceFingerprintChanged,
            });
        }
    }
    Ok(())
}

fn read_interface_deps(
    reader: &mut File,
    deps_count: usize,
) -> Result<Vec<(String, String)>, ModuleCacheLoadError> {
    let mut deps = Vec::with_capacity(deps_count);
    for _ in 0..deps_count {
        let dep_path = read_string(reader).ok_or(ModuleCacheLoadError::CorruptPayload)?;
        let expected_fingerprint =
            read_string(reader).ok_or(ModuleCacheLoadError::CorruptPayload)?;
        deps.push((dep_path, expected_fingerprint));
    }
    Ok(deps)
}

fn reader_ref(reader: &mut File) -> &mut File {
    reader
}

fn write_binding(writer: &mut File, binding: &CachedModuleBinding) -> std::io::Result<()> {
    write_string(writer, &binding.name)?;
    write_u32(writer, binding.index as u32)?;
    write_u32(writer, binding.span.start.line as u32)?;
    write_u32(writer, binding.span.start.column as u32)?;
    write_u32(writer, binding.span.end.line as u32)?;
    write_u32(writer, binding.span.end.column as u32)?;
    writer.write_all(&[u8::from(binding.is_assigned)])?;
    writer.write_all(&[binding.kind as u8])?;
    Ok(())
}

fn read_binding(reader: &mut File) -> Option<CachedModuleBinding> {
    let name = read_string(reader)?;
    let index = read_u32(reader)? as usize;
    let start_line = read_u32(reader)? as usize;
    let start_col = read_u32(reader)? as usize;
    let end_line = read_u32(reader)? as usize;
    let end_col = read_u32(reader)? as usize;
    let mut assigned = [0u8; 1];
    reader.read_exact(&mut assigned).ok()?;
    let mut kind = [0u8; 1];
    reader.read_exact(&mut kind).ok()?;

    Some(CachedModuleBinding {
        name,
        index,
        span: Span::new(
            crate::diagnostics::position::Position::new(start_line, start_col),
            crate::diagnostics::position::Position::new(end_line, end_col),
        ),
        is_assigned: assigned[0] != 0,
        kind: match kind[0] {
            0 => CachedModuleBindingKind::Defined,
            1 => CachedModuleBindingKind::Imported,
            _ => return None,
        },
    })
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::{
        bytecode::compiler::module_interface,
        bytecode::{
            bytecode_cache::{hash_bytes, hash_cache_key},
            debug_info::{EffectSummary, FunctionDebugInfo, InstructionLocation, Location},
        },
        diagnostics::position::{Position, Span},
        runtime::value::Value,
        types::module_interface::ModuleInterface,
    };

    use super::{
        CachedModuleBinding, CachedModuleBindingKind, CachedModuleBytecode, ModuleBytecodeCache,
        ModuleDependencyStatus,
    };

    fn temp_dir(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let pid = std::process::id();
        path.push(format!("flux_module_cache_{}_{}_{}", name, pid, nanos));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn module_cache_roundtrips_module_bytecode() {
        let dir = temp_dir("roundtrip");
        let cache = ModuleBytecodeCache::new(&dir);
        let source_path = PathBuf::from("Example.flx");
        let source_hash = hash_bytes(b"module Example {}");
        let config_hash = hash_bytes(b"strict=0");
        let cache_key = hash_cache_key(&source_hash, &config_hash);
        let payload = CachedModuleBytecode {
            globals: vec![CachedModuleBinding {
                name: "Example.answer".to_string(),
                index: 3,
                span: Span::new(Position::new(1, 0), Position::new(1, 6)),
                is_assigned: true,
                kind: CachedModuleBindingKind::Defined,
            }],
            constants: vec![Value::Integer(42)],
            instructions: vec![1, 2, 3, 4],
            debug_info: FunctionDebugInfo::new(
                None,
                vec!["Example.flx".to_string()],
                vec![InstructionLocation {
                    offset: 0,
                    location: Some(Location {
                        file_id: 0,
                        span: Span::new(Position::new(1, 0), Position::new(1, 6)),
                    }),
                }],
            )
            .with_effect_summary(EffectSummary::Unknown),
        };

        cache
            .store(
                &source_path,
                &cache_key,
                env!("CARGO_PKG_VERSION"),
                &payload,
                &[],
            )
            .unwrap();

        let loaded = cache
            .load(&source_path, &cache_key, env!("CARGO_PKG_VERSION"), &dir)
            .expect("expected module cache hit");

        assert_eq!(loaded, payload);

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn inspect_reports_stale_dependency_interface() {
        let cache_root = std::env::temp_dir().join(format!(
            "flux_module_cache_inspect_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&cache_root);
        std::fs::create_dir_all(&cache_root).expect("cache root");
        let cache_dir = cache_root.join("vm");
        let cache = ModuleBytecodeCache::new(cache_dir);
        let source_path = Path::new("examples/aoc/2024/Day06Solver.flx");
        let cache_key = crate::bytecode::bytecode_cache::hash_bytes(b"cache-key");
        let dep_path = Path::new("lib/Flow/List.flx");
        let interface_path =
            module_interface::interface_path(&cache_root, dep_path);
        std::fs::create_dir_all(interface_path.parent().expect("parent")).expect("mkdirs");
        let mut interface =
            crate::types::module_interface::ModuleInterface::new("Flow.List", "source", "config");
        interface.interface_fingerprint = "actual-fingerprint".to_string();
        module_interface::save_interface(&interface_path, &interface).expect("save interface");

        let bytecode = CachedModuleBytecode {
            globals: Vec::new(),
            constants: Vec::new(),
            instructions: Vec::new(),
            debug_info: FunctionDebugInfo::default(),
        };
        cache
            .store(
                source_path,
                &cache_key,
                env!("CARGO_PKG_VERSION"),
                &bytecode,
                &[(
                    dep_path.to_string_lossy().to_string(),
                    "expected-fingerprint".to_string(),
                )],
            )
            .expect("store");

        let info = cache
            .inspect(
                source_path,
                &cache_key,
                env!("CARGO_PKG_VERSION"),
                &cache_root,
            )
            .expect("inspect");
        assert_eq!(info.dependency_statuses.len(), 1);
        assert_eq!(
            info.dependency_statuses[0].status,
            ModuleDependencyStatus::Stale
        );
    }

    #[test]
    fn module_cache_invalidates_on_dependency_interface_fingerprint_change() {
        let dir = temp_dir("dep_fingerprint");
        let cache = ModuleBytecodeCache::new(&dir);
        let source_path = PathBuf::from("Example.flx");
        let dep_source = PathBuf::from("Dep.flx");
        let source_hash = hash_bytes(b"module Example {}");
        let config_hash = hash_bytes(b"strict=0");
        let cache_key = hash_cache_key(&source_hash, &config_hash);
        let payload = CachedModuleBytecode {
            globals: vec![],
            constants: vec![],
            instructions: vec![1, 2, 3],
            debug_info: FunctionDebugInfo::new(None, vec![], vec![])
                .with_effect_summary(EffectSummary::Unknown),
        };

        let mut dep_interface = ModuleInterface::new("Dep", "source", "config");
        dep_interface.interface_fingerprint = "current-abi".to_string();
        let dep_interface_path = module_interface::interface_path(&dir, &dep_source);
        module_interface::save_interface(&dep_interface_path, &dep_interface).unwrap();

        cache
            .store(
                &source_path,
                &cache_key,
                env!("CARGO_PKG_VERSION"),
                &payload,
                &[(
                    dep_source.to_string_lossy().to_string(),
                    "stale-abi".to_string(),
                )],
            )
            .unwrap();

        let loaded = cache.load(&source_path, &cache_key, env!("CARGO_PKG_VERSION"), &dir);
        assert!(loaded.is_none());

        let reason = cache
            .load_failure_reason(&source_path, &cache_key, env!("CARGO_PKG_VERSION"), &dir)
            .expect("expected cache miss reason");
        assert!(
            reason.contains("dependency mismatch"),
            "expected dependency mismatch reason, got: {reason}"
        );

        std::fs::remove_dir_all(dir).ok();
    }
}
