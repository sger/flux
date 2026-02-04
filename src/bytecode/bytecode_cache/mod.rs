use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use crate::bytecode::bytecode::Bytecode;

mod cache_serialization;
mod cache_validation;

use cache_serialization::{
    read_object, read_string, read_u32, write_function_debug_info, write_object, write_string,
    write_u16, write_u32,
};
use cache_validation::{
    read_deps_and_validate, read_deps_with_status, validate_cache_key, validate_format_version,
    validate_magic,
};

const MAGIC: &[u8; 4] = b"FXBC";
const FORMAT_VERSION: u16 = 3;

pub struct BytecodeCache {
    dir: PathBuf,
}

pub struct CacheInfo {
    pub cache_path: PathBuf,
    pub format_version: u16,
    pub compiler_version: String,
    pub source_hash: [u8; 32],
    pub deps: Vec<(String, [u8; 32], bool)>,
    pub constants_count: usize,
    pub instructions_len: usize,
}

impl BytecodeCache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn load(
        &self,
        source_path: &Path,
        cache_key: &[u8; 32],
        compiler_version: &str,
    ) -> Option<Bytecode> {
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

        let constants_count = read_u32(&mut file)? as usize;
        let mut constants = Vec::with_capacity(constants_count);
        for _ in 0..constants_count {
            constants.push(read_object(&mut file)?);
        }

        let instructions_len = read_u32(&mut file)? as usize;
        let mut instructions = vec![0u8; instructions_len];
        file.read_exact(&mut instructions).ok()?;
        let debug_info = cache_serialization::read_function_debug_info(&mut file);

        Some(Bytecode {
            instructions,
            constants,
            debug_info,
        })
    }

    pub fn inspect(&self, source_path: &Path, cache_key: &[u8; 32]) -> Option<CacheInfo> {
        let path = self.cache_path(source_path, cache_key);
        self.inspect_file(&path)
    }

    pub fn inspect_file(&self, path: &Path) -> Option<CacheInfo> {
        let mut file = File::open(path).ok()?;

        validate_magic(&mut file, MAGIC)?;
        let version = validate_format_version(&mut file, FORMAT_VERSION)?;
        let compiler_version = read_string(&mut file)?;

        let mut cached_source_hash = [0u8; 32];
        file.read_exact(&mut cached_source_hash).ok()?;

        let deps_count = read_u32(&mut file)? as usize;
        let deps = read_deps_with_status(&mut file, deps_count)?;

        let constants_count = read_u32(&mut file)? as usize;
        for _ in 0..constants_count {
            read_object(&mut file)?;
        }

        let instructions_len = read_u32(&mut file)? as usize;

        Some(CacheInfo {
            cache_path: path.to_path_buf(),
            format_version: version,
            compiler_version,
            source_hash: cached_source_hash,
            deps,
            constants_count,
            instructions_len,
        })
    }

    pub fn load_file(&self, path: &Path) -> Option<Bytecode> {
        let mut file = File::open(path).ok()?;

        validate_magic(&mut file, MAGIC)?;
        validate_format_version(&mut file, FORMAT_VERSION)?;
        let _compiler_version = read_string(&mut file)?;

        let mut _source_hash = [0u8; 32];
        file.read_exact(&mut _source_hash).ok()?;

        let deps_count = read_u32(&mut file)? as usize;
        for _ in 0..deps_count {
            let _dep_path = read_string(&mut file)?;
            let mut dep_hash = [0u8; 32];
            file.read_exact(&mut dep_hash).ok()?;
        }

        let constants_count = read_u32(&mut file)? as usize;
        let mut constants = Vec::with_capacity(constants_count);
        for _ in 0..constants_count {
            constants.push(read_object(&mut file)?);
        }

        let instructions_len = read_u32(&mut file)? as usize;
        let mut instructions = vec![0u8; instructions_len];
        file.read_exact(&mut instructions).ok()?;

        let debug_info = cache_serialization::read_function_debug_info(&mut file);

        Some(Bytecode {
            instructions,
            constants,
            debug_info,
        })
    }

    pub fn store(
        &self,
        source_path: &Path,
        cache_key: &[u8; 32],
        compiler_version: &str,
        bytecode: &Bytecode,
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

        write_u32(&mut file, bytecode.constants.len() as u32)?;
        for constant in &bytecode.constants {
            write_object(&mut file, constant)?;
        }

        write_u32(&mut file, bytecode.instructions.len() as u32)?;
        file.write_all(&bytecode.instructions)?;
        write_function_debug_info(&mut file, bytecode.debug_info.as_ref())?;

        Ok(())
    }

    fn cache_path(&self, source_path: &Path, cache_key: &[u8; 32]) -> PathBuf {
        let stem = source_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("module");
        let filename = format!("{}-{}.fxc", stem, to_hex(cache_key));
        self.dir.join(filename)
    }
}

pub use cache_validation::{hash_bytes, hash_cache_key, hash_file};

fn to_hex(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

#[cfg(test)]
mod cache_serialization_test;
#[cfg(test)]
mod cache_validation_test;
