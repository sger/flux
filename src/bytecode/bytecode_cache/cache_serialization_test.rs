use std::{
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom},
    path::PathBuf,
};

use crate::{
    bytecode::debug_info::{FunctionDebugInfo, InstructionLocation, Location},
    frontend::position::{Position, Span},
    runtime::{compiled_function::CompiledFunction, object::Object},
};

use super::cache_serialization::{
    read_function_debug_info, read_object, read_string, read_u16, read_u32,
    write_function_debug_info, write_object, write_string, write_u16, write_u32,
};

fn temp_path(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let pid = std::process::id();
    path.push(format!("flux_{}_{}_{}", name, pid, nanos));
    path
}

fn create_rw_file(path: &PathBuf) -> File {
    OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(path)
        .unwrap()
}

#[test]
fn write_read_primitives() {
    let path = temp_path("cache_serialization_primitives");
    let mut file = create_rw_file(&path);

    write_u16(&mut file, 42).unwrap();
    write_u32(&mut file, 9001).unwrap();
    write_string(&mut file, "hello").unwrap();

    file.seek(SeekFrom::Start(0)).unwrap();

    assert_eq!(read_u16(&mut file), Some(42));
    assert_eq!(read_u32(&mut file), Some(9001));
    assert_eq!(read_string(&mut file), Some("hello".to_string()));

    fs::remove_file(path).ok();
}

#[test]
fn object_roundtrip_includes_function_debug_info() {
    let path = temp_path("cache_serialization_object");
    let mut file = create_rw_file(&path);

    let debug_info = FunctionDebugInfo::new(
        Some("foo".to_string()),
        vec!["main.flx".to_string()],
        vec![InstructionLocation {
            offset: 0,
            location: Some(Location {
                file_id: 0,
                span: Span::new(Position::new(1, 0), Position::new(1, 3)),
            }),
        }],
    );

    let function = CompiledFunction::new(vec![1, 2, 3], 2, 1, Some(debug_info.clone()));

    let objects = vec![
        Object::Integer(7),
        Object::Float(3.5),
        Object::String("ok".to_string()),
        Object::Function(std::rc::Rc::new(function)),
    ];

    for obj in &objects {
        write_object(&mut file, obj).unwrap();
    }

    file.seek(SeekFrom::Start(0)).unwrap();

    let mut read_back = Vec::new();
    for _ in 0..objects.len() {
        read_back.push(read_object(&mut file).unwrap());
    }

    assert_eq!(read_back, objects);

    fs::remove_file(path).ok();
}

#[test]
fn function_debug_info_roundtrip() {
    let path = temp_path("cache_serialization_debug_info");
    let mut file = create_rw_file(&path);

    let debug_info = FunctionDebugInfo::new(
        Some("bar".to_string()),
        vec!["file.flx".to_string(), "dep.flx".to_string()],
        vec![InstructionLocation {
            offset: 2,
            location: Some(Location {
                file_id: 1,
                span: Span::new(Position::new(2, 4), Position::new(2, 8)),
            }),
        }],
    );

    write_function_debug_info(&mut file, Some(&debug_info)).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();

    let read_back = read_function_debug_info(&mut file).unwrap();
    assert_eq!(read_back, debug_info);

    fs::remove_file(path).ok();
}
