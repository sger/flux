use std::{
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::PathBuf,
};

use super::{
    cache_serialization::{read_string, write_string, write_u16, write_u32},
    cache_validation::{
        hash_bytes, hash_cache_key, hash_file, read_deps_and_validate, read_deps_with_status,
        validate_cache_key, validate_format_version, validate_magic,
    },
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
fn validate_magic_and_version() {
    let path = temp_path("cache_validation_magic");
    let mut file = create_rw_file(&path);

    file.write_all(b"FXBC").unwrap();
    write_u16(&mut file, 3).unwrap();

    file.seek(SeekFrom::Start(0)).unwrap();

    assert!(validate_magic(&mut file, b"FXBC").is_some());
    assert_eq!(validate_format_version(&mut file, 3), Some(3));

    fs::remove_file(path).ok();
}

#[test]
fn validate_cache_key_matches() {
    let path = temp_path("cache_validation_key");
    let mut file = create_rw_file(&path);

    let key = [7u8; 32];
    file.write_all(&key).unwrap();

    file.seek(SeekFrom::Start(0)).unwrap();
    assert!(validate_cache_key(&mut file, &key).is_some());

    fs::remove_file(path).ok();
}

#[test]
fn hash_helpers_are_stable() {
    let a = hash_bytes(b"alpha");
    let a_again = hash_bytes(b"alpha");
    let b = hash_bytes(b"beta");

    assert_eq!(a, a_again);
    assert_ne!(a, b);

    let cache_key = hash_cache_key(&a, &b);
    assert_eq!(cache_key.len(), 32);
}

#[test]
fn hash_file_matches_hash_bytes() {
    let path = temp_path("cache_validation_hash_file");
    let mut file = create_rw_file(&path);
    file.write_all(b"content").unwrap();

    let expected = hash_bytes(b"content");
    let actual = hash_file(&path).unwrap();

    assert_eq!(actual, expected);

    fs::remove_file(path).ok();
}

#[test]
fn read_deps_and_validate_success() {
    let dep_path = temp_path("cache_validation_dep");
    let mut dep_file = create_rw_file(&dep_path);
    dep_file.write_all(b"dep").unwrap();
    dep_file.sync_all().unwrap();

    let dep_hash = hash_file(&dep_path).unwrap();

    let path = temp_path("cache_validation_deps_ok");
    let mut file = create_rw_file(&path);

    write_string(&mut file, dep_path.to_string_lossy().as_ref()).unwrap();
    file.write_all(&dep_hash).unwrap();

    file.seek(SeekFrom::Start(0)).unwrap();
    assert!(read_deps_and_validate(&mut file, 1).is_some());

    fs::remove_file(path).ok();
    fs::remove_file(dep_path).ok();
}

#[test]
fn read_deps_and_validate_failure() {
    let dep_path = temp_path("cache_validation_dep_bad");
    let mut dep_file = create_rw_file(&dep_path);
    dep_file.write_all(b"dep").unwrap();
    dep_file.sync_all().unwrap();

    let path = temp_path("cache_validation_deps_fail");
    let mut file = create_rw_file(&path);

    write_string(&mut file, dep_path.to_string_lossy().as_ref()).unwrap();
    file.write_all(&[0u8; 32]).unwrap();

    file.seek(SeekFrom::Start(0)).unwrap();
    assert!(read_deps_and_validate(&mut file, 1).is_none());

    fs::remove_file(path).ok();
    fs::remove_file(dep_path).ok();
}

#[test]
fn read_deps_with_status_reports_validity() {
    let dep_path = temp_path("cache_validation_dep_status");
    let mut dep_file = create_rw_file(&dep_path);
    dep_file.write_all(b"dep").unwrap();
    dep_file.sync_all().unwrap();

    let dep_hash = hash_file(&dep_path).unwrap();

    let path = temp_path("cache_validation_deps_status");
    let mut file = create_rw_file(&path);

    write_string(&mut file, dep_path.to_string_lossy().as_ref()).unwrap();
    file.write_all(&dep_hash).unwrap();

    file.seek(SeekFrom::Start(0)).unwrap();
    let deps = read_deps_with_status(&mut file, 1).unwrap();

    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].0, dep_path.to_string_lossy().to_string());
    assert!(deps[0].2);

    fs::remove_file(path).ok();
    fs::remove_file(dep_path).ok();
}

#[test]
fn read_string_roundtrip_in_validation_helpers() {
    let path = temp_path("cache_validation_read_string");
    let mut file = create_rw_file(&path);

    write_u32(&mut file, 3).unwrap();
    file.write_all(b"abc").unwrap();

    file.seek(SeekFrom::Start(0)).unwrap();
    assert_eq!(read_string(&mut file), Some("abc".to_string()));

    fs::remove_file(path).ok();
}
