use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{
    bytecode::bytecode::Bytecode, runtime::compiled_function::CompiledFunction,
    runtime::object::Object,
};

const MAGIC: &[u8; 4] = b"FXBC";
const FORMAT_VERSION: u16 = 1;

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
        source_hash: &[u8; 32],
        compiler_version: &str,
    ) -> Option<Bytecode> {
        let path = self.cache_path(source_path, source_hash);
        let mut file = File::open(path).ok()?;

        let mut magic = [0u8; 4];
        file.read_exact(&mut magic).ok()?;
        if &magic != MAGIC {
            return None;
        }

        let version = read_u16(&mut file)?;
        if version != FORMAT_VERSION {
            return None;
        }

        let cached_compiler_version = read_string(&mut file)?;
        if cached_compiler_version != compiler_version {
            return None;
        }

        let mut cached_source_hash = [0u8; 32];
        file.read_exact(&mut cached_source_hash).ok()?;
        if &cached_source_hash != source_hash {
            return None;
        }

        let deps_count = read_u32(&mut file)? as usize;
        for _ in 0..deps_count {
            let dep_path = read_string(&mut file)?;
            let mut dep_hash = [0u8; 32];
            file.read_exact(&mut dep_hash).ok()?;
            if hash_file(Path::new(&dep_path)).ok()? != dep_hash {
                return None;
            }
        }

        let constants_count = read_u32(&mut file)? as usize;
        let mut constants = Vec::with_capacity(constants_count);
        for _ in 0..constants_count {
            constants.push(read_object(&mut file)?);
        }

        let instructions_len = read_u32(&mut file)? as usize;
        let mut instructions = vec![0u8; instructions_len];
        file.read_exact(&mut instructions).ok()?;

        Some(Bytecode {
            instructions,
            constants,
        })
    }

    pub fn inspect(&self, source_path: &Path, source_hash: &[u8; 32]) -> Option<CacheInfo> {
        let path = self.cache_path(source_path, source_hash);
        self.inspect_file(&path)
    }

    pub fn inspect_file(&self, path: &Path) -> Option<CacheInfo> {
        let mut file = File::open(path).ok()?;

        let mut magic = [0u8; 4];
        file.read_exact(&mut magic).ok()?;
        if &magic != MAGIC {
            return None;
        }

        let version = read_u16(&mut file)?;
        let compiler_version = read_string(&mut file)?;

        let mut cached_source_hash = [0u8; 32];
        file.read_exact(&mut cached_source_hash).ok()?;

        let deps_count = read_u32(&mut file)? as usize;
        let mut deps = Vec::with_capacity(deps_count);
        for _ in 0..deps_count {
            let dep_path = read_string(&mut file)?;
            let mut dep_hash = [0u8; 32];
            file.read_exact(&mut dep_hash).ok()?;
            let valid = hash_file(Path::new(&dep_path))
                .ok()
                .map(|current| current == dep_hash)
                .unwrap_or(false);
            deps.push((dep_path, dep_hash, valid));
        }

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

        let mut magic = [0u8; 4];
        file.read_exact(&mut magic).ok()?;
        if &magic != MAGIC {
            return None;
        }

        let _version = read_u16(&mut file)?;
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

        Some(Bytecode {
            instructions,
            constants,
        })
    }

    pub fn store(
        &self,
        source_path: &Path,
        source_hash: &[u8; 32],
        compiler_version: &str,
        bytecode: &Bytecode,
        deps: &[(String, [u8; 32])],
    ) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.cache_path(source_path, source_hash);
        let mut file = File::create(path)?;

        file.write_all(MAGIC)?;
        write_u16(&mut file, FORMAT_VERSION)?;
        write_string(&mut file, compiler_version)?;
        file.write_all(source_hash)?;

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

        Ok(())
    }

    fn cache_path(&self, source_path: &Path, source_hash: &[u8; 32]) -> PathBuf {
        let stem = source_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("module");
        let filename = format!("{}-{}.fxc", stem, to_hex(source_hash));
        self.dir.join(filename)
    }
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

fn to_hex(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

fn write_u16(writer: &mut File, value: u16) -> std::io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn write_u32(writer: &mut File, value: u32) -> std::io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn write_i64(writer: &mut File, value: i64) -> std::io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn write_f64(writer: &mut File, value: f64) -> std::io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn write_string(writer: &mut File, value: &str) -> std::io::Result<()> {
    let bytes = value.as_bytes();
    write_u32(writer, bytes.len() as u32)?;
    writer.write_all(bytes)
}

fn read_u16(reader: &mut File) -> Option<u16> {
    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf).ok()?;
    Some(u16::from_le_bytes(buf))
}

fn read_u32(reader: &mut File) -> Option<u32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf).ok()?;
    Some(u32::from_le_bytes(buf))
}

fn read_i64(reader: &mut File) -> Option<i64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf).ok()?;
    Some(i64::from_le_bytes(buf))
}

fn read_f64(reader: &mut File) -> Option<f64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf).ok()?;
    Some(f64::from_le_bytes(buf))
}

fn read_string(reader: &mut File) -> Option<String> {
    let len = read_u32(reader)? as usize;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).ok()?;
    String::from_utf8(buf).ok()
}

fn write_object(writer: &mut File, obj: &Object) -> std::io::Result<()> {
    match obj {
        Object::Integer(value) => {
            writer.write_all(&[0])?;
            write_i64(writer, *value)
        }
        Object::Float(value) => {
            writer.write_all(&[1])?;
            write_f64(writer, *value)
        }
        Object::String(value) => {
            writer.write_all(&[2])?;
            write_string(writer, value)
        }
        Object::Function(func) => {
            writer.write_all(&[3])?;
            write_u16(writer, func.num_locals as u16)?;
            write_u16(writer, func.num_parameters as u16)?;
            write_u32(writer, func.instructions.len() as u32)?;
            writer.write_all(&func.instructions)
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unsupported constant type: {}", obj.type_name()),
        )),
    }
}

fn read_object(reader: &mut File) -> Option<Object> {
    let mut tag = [0u8; 1];
    reader.read_exact(&mut tag).ok()?;
    match tag[0] {
        0 => Some(Object::Integer(read_i64(reader)?)),
        1 => Some(Object::Float(read_f64(reader)?)),
        2 => Some(Object::String(read_string(reader)?)),
        3 => {
            let num_locals = read_u16(reader)? as usize;
            let num_parameters = read_u16(reader)? as usize;
            let instructions_len = read_u32(reader)? as usize;
            let mut instructions = vec![0u8; instructions_len];
            reader.read_exact(&mut instructions).ok()?;
            Some(Object::Function(std::rc::Rc::new(CompiledFunction::new(
                instructions,
                num_locals,
                num_parameters,
            ))))
        }
        _ => None,
    }
}
