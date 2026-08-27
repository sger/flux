//! Integration tests for `Flow.Crypto` (proposal 0178, item 4).
//!
//! SHA-256 is implemented twice and shares no code between the two: the VM
//! calls the `sha2` crate, the native backend runs a hand-written C
//! implementation in `runtime/c/sha256.c`. A divergence between them is a
//! *silent wrong answer* rather than a crash, which makes the published NIST
//! vectors the load-bearing assertion here — and makes it worth checking the
//! digests against the compiler's own `sha2` output rather than only against
//! constants copied into the fixture.
//!
//! `sha256` is also the first pure primop added by this proposal, so its lack
//! of an effect is asserted too: it must be callable from a function with no
//! effect annotation, while `sha256_file` must be rejected without
//! `FileSystem`.

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn scratch_dir() -> PathBuf {
    let dir = workspace_root().join("target").join("test-scratch");
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn run_flux_test(fixture: &str) -> (String, bool) {
    let path = workspace_root().join("tests").join("flux").join(fixture);
    let scratch = Scratch::new("cache-isolated");
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["--test", path.to_str().unwrap(), "--no-cache"])
        .args(scratch.cache_args())
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --test on {fixture}: {e}"));
    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    (stdout, output.status.success())
}

fn run_source(name: &str, source: &str) -> (String, String, bool) {
    let file = scratch_dir().join(name);
    std::fs::write(&file, source).expect("write scratch fixture");
    let scratch = Scratch::new("cache-isolated");
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["run", file.to_str().unwrap(), "--no-cache"])
        .args(scratch.cache_args())
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux on {name}: {e}"));
    let _ = std::fs::remove_file(&file);
    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

#[test]
fn stdlib_crypto_flux_suite_passes() {
    let (stdout, success) = run_flux_test("stdlib_crypto.flx");
    assert!(success, "Flow.Crypto test suite failed:\n{stdout}");
    assert!(
        stdout.contains("16 tests: 16 passed, 0 failed"),
        "expected all 16 Flow.Crypto tests to pass, got:\n{stdout}"
    );
}

/// Cross-check Flux's digests against the compiler's own `sha2`, rather than
/// only against constants pasted into a fixture. A typo in a hard-coded vector
/// would make a wrong implementation look correct; this cannot.
#[test]
fn digests_match_the_compilers_own_sha2() {
    use sha2::{Digest, Sha256};

    let inputs = ["", "abc", "hello world", "a longer string with spaces"];
    let program = inputs
        .iter()
        .map(|s| format!(r#"    println(Crypto.sha256("{s}"))"#))
        .collect::<Vec<_>>()
        .join("\n");

    let (stdout, stderr, success) = run_source(
        "crypto_crosscheck.flx",
        &format!(
            r#"
import Flow.Crypto as Crypto

fn main() -> Unit with Console {{
{program}
}}
"#
        ),
    );
    assert!(success, "run failed:\n{stdout}\n{stderr}");

    for input in inputs {
        let mut hasher = Sha256::new();
        hasher.update(input.as_bytes());
        let expected = flux::shared::hex::encode(&hasher.finalize());
        assert!(
            stdout.contains(&expected),
            "digest for {input:?} should be {expected}, got:\n{stdout}"
        );
    }
}

/// Hashing a file must equal hashing its bytes. The file path streams in
/// chunks while the string path hashes in one shot, so this pins the two
/// against each other across a size that spans several 64 KiB reads.
#[test]
fn hashing_a_large_file_streams_to_the_same_digest() {
    use sha2::{Digest, Sha256};

    let path = scratch_dir().join("crypto_large.bin");
    // Deliberately not a multiple of the 64 KiB chunk size, so the final
    // partial read and the padding path are both exercised.
    let payload = vec![b'a'; 200_003];
    std::fs::write(&path, &payload).expect("write large fixture");

    let mut hasher = Sha256::new();
    hasher.update(&payload);
    let expected = flux::shared::hex::encode(&hasher.finalize());

    let (stdout, stderr, success) = run_source(
        "crypto_large.flx",
        r#"
import Flow.Crypto as Crypto

fn main() -> Unit with FileSystem, Console {
    match Crypto.sha256_file("target/test-scratch/crypto_large.bin") {
        Ok(h) -> println(h),
        Err(_) -> println("ERR"),
    }
}
"#,
    );
    let _ = std::fs::remove_file(&path);
    assert!(success, "run failed:\n{stdout}\n{stderr}");
    assert!(
        stdout.contains(&expected),
        "streamed digest should be {expected}, got:\n{stdout}"
    );
}

/// The headline property of `sha256`: it is pure. A function with no effect
/// annotation may call it, which is what lets a manifest parser be provably
/// pure while the fetcher feeding it wears its I/O in its type.
#[test]
fn sha256_is_pure_and_needs_no_effect_annotation() {
    let (stdout, stderr, success) = run_source(
        "crypto_pure.flx",
        r#"
import Flow.Crypto as Crypto

fn fingerprint(name: String) -> String {
    Crypto.sha256(name)
}

fn main() -> Unit with Console {
    println(fingerprint("abc"))
}
"#,
    );
    assert!(
        success,
        "sha256 must be callable from a pure function:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        "got:\n{stdout}"
    );
}

/// `sha256_file` touches the filesystem and must say so, unlike `sha256`.
#[test]
fn sha256_file_requires_the_filesystem_effect() {
    let (stdout, stderr, success) = run_source(
        "crypto_effect.flx",
        r#"
import Flow.Crypto as Crypto
import Flow.Result as Result

fn sneaky() -> Bool {
    Result.is_ok(Crypto.sha256_file("Cargo.toml"))
}

fn main() -> Unit with Console {
    println(to_string(sneaky()))
}
"#,
    );
    assert!(
        !success,
        "an undeclared FileSystem effect must be rejected:\n{stdout}"
    );
    assert!(
        format!("{stdout}{stderr}").contains("FileSystem"),
        "the diagnostic should name the FileSystem effect"
    );
}

/// Failure is a value here too: hashing a missing file returns `Err` with a
/// classified kind and the attempted path, and the program keeps running.
#[test]
fn hashing_a_missing_file_is_recoverable() {
    let (stdout, stderr, success) = run_source(
        "crypto_missing.flx",
        r#"
import Flow.Crypto as Crypto
import Flow.IoError as Io

fn main() -> Unit with FileSystem, Console {
    match Crypto.sha256_file("/nonexistent/nope.bin") {
        Ok(_) -> println("unexpected"),
        Err(e) -> do {
            println("kind=" + Io.kind_name(Io.error_kind(e)))
            println("path=" + Io.error_path(e))
        },
    }
    println("still running")
}
"#,
    );
    assert!(success, "run failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("kind=NotFound"), "got:\n{stdout}");
    assert!(
        stdout.contains("path=/nonexistent/nope.bin"),
        "got:\n{stdout}"
    );
    assert!(
        stdout.contains("still running"),
        "a failed hash must not abort:\n{stdout}"
    );
}
