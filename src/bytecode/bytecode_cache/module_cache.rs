use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use crate::{
    bytecode::{
        bytecode_cache::cache_serialization::{
            read_function_debug_info, read_object, read_string, read_u32,
            write_function_debug_info, write_object, write_string, write_u16, write_u32,
        },
        bytecode_cache::cache_validation::{
            read_deps_and_validate, validate_cache_key, validate_format_version, validate_magic,
        },
        debug_info::FunctionDebugInfo,
    },
    diagnostics::position::Span,
    runtime::value::Value,
};

const MAGIC: &[u8; 4] = b"FXMC";
const FORMAT_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct CachedModuleBinding {
    pub name: String,
    pub index: usize,
    pub span: Span,
    pub is_assigned: bool,
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
    ) -> Option<CachedModuleBytecode> {
        let path = self.cache_path(source_path, cache_key);
        let mut file = File::open(path).ok()?;

        validate_magic(&mut file, MAGIC)?;
        validate_format_version(&mut file, FORMAT_VERSION)?;

        let cached_compiler_version = read_string(&mut file)?;
        if cached_compiler_version != compiler_version {
            return None;
        }

        validate_cache_key(&mut file, cache_key)?;

        let deps_count = read_u32(&mut file)? as usize;
        read_deps_and_validate(&mut file, deps_count)?;

        let globals_count = read_u32(&mut file)? as usize;
        let mut globals = Vec::with_capacity(globals_count);
        for _ in 0..globals_count {
            globals.push(read_binding(&mut file)?);
        }

        let constants_count = read_u32(&mut file)? as usize;
        let mut constants = Vec::with_capacity(constants_count);
        for _ in 0..constants_count {
            constants.push(read_object(&mut file)?);
        }

        let instructions_len = read_u32(&mut file)? as usize;
        let mut instructions = vec![0u8; instructions_len];
        file.read_exact(&mut instructions).ok()?;

        let debug_info = read_function_debug_info(&mut file)?;

        Some(CachedModuleBytecode {
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
        deps: &[(String, [u8; 32])],
    ) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.cache_path(source_path, cache_key);
        let mut file = File::create(path)?;

        file.write_all(MAGIC)?;
        write_u16(&mut file, FORMAT_VERSION)?;
        write_string(&mut file, compiler_version)?;
        file.write_all(cache_key)?;

        write_u32(&mut file, deps.len() as u32)?;
        for (dep_path, dep_hash) in deps {
            write_string(&mut file, dep_path)?;
            file.write_all(dep_hash)?;
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

    fn cache_path(&self, source_path: &Path, cache_key: &[u8; 32]) -> PathBuf {
        let stem = source_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("module");
        let filename = format!("{}-{}.fxm", stem, super::to_hex(cache_key));
        self.dir.join(filename)
    }
}

fn write_binding(writer: &mut File, binding: &CachedModuleBinding) -> std::io::Result<()> {
    write_string(writer, &binding.name)?;
    write_u32(writer, binding.index as u32)?;
    write_u32(writer, binding.span.start.line as u32)?;
    write_u32(writer, binding.span.start.column as u32)?;
    write_u32(writer, binding.span.end.line as u32)?;
    write_u32(writer, binding.span.end.column as u32)?;
    writer.write_all(&[u8::from(binding.is_assigned)])?;
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

    Some(CachedModuleBinding {
        name,
        index,
        span: Span::new(
            crate::diagnostics::position::Position::new(start_line, start_col),
            crate::diagnostics::position::Position::new(end_line, end_col),
        ),
        is_assigned: assigned[0] != 0,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::{
        bytecode::{
            bytecode_cache::{hash_bytes, hash_cache_key},
            debug_info::{EffectSummary, FunctionDebugInfo, InstructionLocation, Location},
        },
        diagnostics::position::{Position, Span},
        runtime::value::Value,
    };

    use super::{CachedModuleBinding, CachedModuleBytecode, ModuleBytecodeCache};

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
            .load(&source_path, &cache_key, env!("CARGO_PKG_VERSION"))
            .expect("expected module cache hit");

        assert_eq!(loaded, payload);

        std::fs::remove_dir_all(dir).ok();
    }
}
