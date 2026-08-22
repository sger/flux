//! Integration tests for `Flow.Fs` and the `TryReadFile` primop
//! (proposal 0178, first fallible primop).
//!
//! This is the first primop that reports failure as a value rather than
//! aborting, so what is asserted here is the *shape* of that contract:
//!
//!   * a missing file produces `Err(IoError { kind: NotFound, .. })` and the
//!     program keeps running — the old `read_file` would have aborted;
//!   * the error carries the path that was attempted;
//!   * the VM and the native backend classify the same failure identically.
//!     The two implementations classify independently — the VM matches on
//!     Rust's `io::ErrorKind`, the C runtime on `errno` — so agreement is a
//!     real invariant and not a shared code path.

use std::path::{Path, PathBuf};
use std::process::Command;

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
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["--test", path.to_str().unwrap(), "--no-cache"])
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
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["run", file.to_str().unwrap(), "--no-cache"])
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
fn stdlib_fs_flux_suite_passes() {
    let (stdout, success) = run_flux_test("stdlib_fs.flx");
    assert!(success, "Flow.Fs test suite failed:\n{stdout}");
    assert!(
        stdout.contains("31 tests: 31 passed, 0 failed"),
        "expected all 31 Flow.Fs tests to pass, got:\n{stdout}"
    );
}

/// The headline behavioural change: a read that fails no longer stops the
/// program. The old `read_file` aborts on a missing path; this one returns and
/// the following statement still runs.
#[test]
fn a_failed_read_does_not_abort_the_program() {
    let (stdout, stderr, success) = run_source(
        "fs_no_abort.flx",
        r#"
import Flow.Fs as Fs
import Flow.IoError as Io

fn main() -> Unit with IO {
    match Fs.read_file("/nonexistent/nope.txt") {
        Ok(_) -> println("unexpected"),
        Err(e) -> println("handled " + Io.kind_name(Io.error_kind(e))),
    }
    println("still running")
}
"#,
    );
    assert!(
        success,
        "a failed read must not abort:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("handled NotFound"),
        "expected handled NotFound:\n{stdout}"
    );
    assert!(
        stdout.contains("still running"),
        "execution should continue after a failed read:\n{stdout}"
    );
}

/// The error names the path it tried, which is what makes a multi-path
/// fallback loop diagnosable.
#[test]
fn the_error_reports_the_attempted_path() {
    let (stdout, stderr, success) = run_source(
        "fs_error_path.flx",
        r#"
import Flow.Fs as Fs
import Flow.IoError as Io

fn main() -> Unit with IO {
    match Fs.read_file("/nonexistent/named.txt") {
        Ok(_) -> println("unexpected"),
        Err(e) -> println("path=" + Io.error_path(e)),
    }
}
"#,
    );
    assert!(success, "run failed:\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("path=/nonexistent/named.txt"),
        "expected the attempted path in the error:\n{stdout}"
    );
}

/// Reading a real file still works — the failure path did not replace the
/// success path.
#[test]
fn reading_an_existing_file_returns_its_contents() {
    let fixture = scratch_dir().join("fs_read_me.txt");
    std::fs::write(&fixture, "hello fs").expect("write fixture");

    let (stdout, stderr, success) = run_source(
        "fs_read_ok.flx",
        &format!(
            r#"
import Flow.Fs as Fs

fn main() -> Unit with IO {{
    match Fs.read_file("{}") {{
        Ok(c) -> println("got:" + c),
        Err(_) -> println("unexpected error"),
    }}
}}
"#,
            fixture.to_str().unwrap()
        ),
    );
    let _ = std::fs::remove_file(&fixture);
    assert!(success, "run failed:\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("got:hello fs"),
        "expected got:hello fs:\n{stdout}"
    );
}

/// `read_file_or` collapses the `Result` for the "optional config" shape.
#[test]
fn read_file_or_falls_back_without_the_caller_seeing_an_error() {
    let (stdout, stderr, success) = run_source(
        "fs_read_or.flx",
        r#"
import Flow.Fs as Fs

fn main() -> Unit with IO {
    println(Fs.read_file_or("/nonexistent/cfg.toml", "defaulted"))
}
"#,
    );
    assert!(success, "run failed:\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("defaulted"),
        "expected defaulted:\n{stdout}"
    );
}

/// `Flow.Fs.read_file` carries `FileSystem`, so a caller that does not declare
/// the effect must be rejected. Losing this would make the capability
/// invisible in signatures — the property 0178 exists to provide.
#[test]
fn reading_a_file_requires_the_filesystem_effect() {
    let (stdout, stderr, success) = run_source(
        "fs_effect_required.flx",
        r#"
import Flow.Fs as Fs

fn sneaky(path: String) -> Bool {
    match Fs.read_file(path) {
        Ok(_) -> true,
        Err(_) -> false,
    }
}

fn main() -> Unit with IO {
    println(to_string(sneaky("Cargo.toml")))
}
"#,
    );
    assert!(
        !success,
        "an undeclared FileSystem effect must be rejected:\nstdout:\n{stdout}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("FileSystem"),
        "the diagnostic should name the FileSystem effect:\n{combined}"
    );
}

/// Predicates answer `Bool`, and answer `false` rather than erroring for a
/// path that does not exist.
#[test]
fn predicates_distinguish_files_directories_and_absence() {
    let (stdout, stderr, success) = run_source(
        "fs_predicates.flx",
        r#"
import Flow.Fs as Fs

fn main() -> Unit with IO {
    println("file=" + to_string(Fs.is_file("Cargo.toml")))
    println("dir=" + to_string(Fs.is_dir("src")))
    println("missing=" + to_string(Fs.exists("/nonexistent/x")))
    println("dir_not_file=" + to_string(Fs.is_file("src")))
}
"#,
    );
    assert!(success, "run failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("file=true"), "{stdout}");
    assert!(stdout.contains("dir=true"), "{stdout}");
    assert!(stdout.contains("missing=false"), "{stdout}");
    assert!(stdout.contains("dir_not_file=false"), "{stdout}");
}

/// A full create/write/rename/remove cycle, asserting the tree is gone at the
/// end. Exercises every mutation in one program.
#[test]
fn a_full_write_rename_remove_cycle_leaves_nothing_behind() {
    let base = "target/test-scratch/fs_cycle";
    let _ = std::fs::remove_dir_all(workspace_root().join(base));

    let (stdout, stderr, success) = run_source(
        "fs_cycle.flx",
        &format!(
            r#"
import Flow.Fs as Fs
import Flow.Result as Result

fn main() -> Unit with IO {{
    let base = "{base}"
    println("mkdir=" + to_string(Result.is_ok(Fs.create_dir_all(base + "/sub"))))
    println("write=" + to_string(Result.is_ok(Fs.write_file(base + "/sub/a.txt", "v"))))
    println("rename=" + to_string(Result.is_ok(Fs.rename(base + "/sub/a.txt", base + "/sub/b.txt"))))
    println("content=" + Fs.read_file_or(base + "/sub/b.txt", "MISSING"))
    println("rmtree=" + to_string(Result.is_ok(Fs.remove_dir_all(base))))
    println("gone=" + to_string(Fs.exists(base) == false))
}}
"#
        ),
    );
    assert!(success, "run failed:\n{stdout}\n{stderr}");
    for expected in [
        "mkdir=true",
        "write=true",
        "rename=true",
        "content=v",
        "rmtree=true",
        "gone=true",
    ] {
        assert!(stdout.contains(expected), "missing {expected}:\n{stdout}");
    }
    assert!(
        !workspace_root().join(base).exists(),
        "remove_dir_all left the tree behind"
    );
}

/// Errors are classified the same way on both backends. The VM matches on
/// Rust's `io::ErrorKind`; the C runtime matches on `errno`. Nothing shares
/// code, so agreement has to be asserted.
#[test]
fn every_mutation_reports_not_found_for_a_missing_path() {
    let (stdout, stderr, success) = run_source(
        "fs_missing_kinds.flx",
        r#"
import Flow.Fs as Fs
import Flow.IoError as Io
import Flow.Result as Result

fn kind(r: Result<Unit, IoError>) -> String {
    match r {
        Ok(_) -> "ok",
        Err(e) -> Io.kind_name(Io.error_kind(e)),
    }
}

fn main() -> Unit with IO {
    println("write=" + kind(Fs.write_file("/nonexistent/d/f.txt", "x")))
    println("rmfile=" + kind(Fs.remove_file("/nonexistent/f.txt")))
    println("rmdir=" + kind(Fs.remove_dir_all("/nonexistent/d")))
    println("rename=" + kind(Fs.rename("/nonexistent/a", "/nonexistent/b")))
}
"#,
    );
    assert!(success, "run failed:\n{stdout}\n{stderr}");
    for expected in [
        "write=NotFound",
        "rmfile=NotFound",
        "rmdir=NotFound",
        "rename=NotFound",
    ] {
        assert!(stdout.contains(expected), "missing {expected}:\n{stdout}");
    }
}

/// Mutations carry `FileSystem` too — a caller that does not declare it must
/// be rejected, the same as for reads.
#[test]
fn writing_a_file_requires_the_filesystem_effect() {
    let (stdout, stderr, success) = run_source(
        "fs_write_effect.flx",
        r#"
import Flow.Fs as Fs
import Flow.Result as Result

fn sneaky() -> Bool {
    Result.is_ok(Fs.write_file("target/test-scratch/sneaky.txt", "x"))
}

fn main() -> Unit with IO {
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

/// `list_dir` reports exactly what was created — no more (`.`/`..` are not
/// entries) and no less. Names are bare, so joining is the caller's job.
#[test]
fn list_dir_returns_bare_entry_names() {
    let base = "target/test-scratch/fs_rs_list";
    let _ = std::fs::remove_dir_all(workspace_root().join(base));
    let (stdout, stderr, success) = run_source(
        "fs_list_dir.flx",
        &format!(
            r#"
import Flow.Fs as Fs
import Flow.Array as Array
import Flow.Result as Result

fn main() -> Unit with IO {{
    let base = "{base}"
    println("mkdir=" + to_string(Result.is_ok(Fs.create_dir_all(base))))
    println("w1=" + to_string(Result.is_ok(Fs.write_file(base + "/a.txt", "1"))))
    println("w2=" + to_string(Result.is_ok(Fs.write_file(base + "/b.txt", "2"))))
    match Fs.list_dir(base) {{
        Ok(names) -> do {{
            println("count=" + to_string(len(names)))
            println("has_a=" + to_string(Array.contains(names, "a.txt")))
            println("has_b=" + to_string(Array.contains(names, "b.txt")))
            println("no_dot=" + to_string(Array.contains(names, ".") == false))
            println("bare=" + to_string(Array.contains(names, base + "/a.txt") == false))
        }},
        Err(_) -> println("count=ERR"),
    }}
}}
"#
        ),
    );
    let _ = std::fs::remove_dir_all(workspace_root().join(base));
    assert!(success, "run failed:\n{stdout}\n{stderr}");
    for expected in [
        "count=2",
        "has_a=true",
        "has_b=true",
        "no_dot=true",
        "bare=true",
    ] {
        assert!(stdout.contains(expected), "missing {expected}:\n{stdout}");
    }
}

/// Listing failures are classified, not collapsed into one error. A missing
/// directory and a path that is a file fail for different reasons, and the
/// VM (`io::ErrorKind`) and C runtime (`errno`) must agree on both.
#[test]
fn list_dir_distinguishes_missing_from_not_a_directory() {
    let (stdout, stderr, success) = run_source(
        "fs_list_kinds.flx",
        r#"
import Flow.Fs as Fs
import Flow.IoError as Io

fn kind(r: Result<Array<String>, IoError>) -> String {
    match r {
        Ok(_) -> "ok",
        Err(e) -> Io.kind_name(Io.error_kind(e)),
    }
}

fn main() -> Unit with IO {
    println("missing=" + kind(Fs.list_dir("/nonexistent/d")))
    println("notdir=" + kind(Fs.list_dir("Cargo.toml")))
}
"#,
    );
    assert!(success, "run failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("missing=NotFound"), "got:\n{stdout}");
    assert!(stdout.contains("notdir=NotADirectory"), "got:\n{stdout}");
}

/// `metadata` answers several questions from one syscall, and its answers
/// agree with the standalone predicates.
#[test]
fn metadata_reports_size_and_kind_consistently_with_the_predicates() {
    let base = "target/test-scratch/fs_rs_meta";
    let _ = std::fs::remove_dir_all(workspace_root().join(base));
    let (stdout, stderr, success) = run_source(
        "fs_metadata.flx",
        &format!(
            r#"
import Flow.Fs as Fs
import Flow.Result as Result

fn main() -> Unit with IO {{
    let base = "{base}"
    println("mkdir=" + to_string(Result.is_ok(Fs.create_dir_all(base))))
    println("write=" + to_string(Result.is_ok(Fs.write_file(base + "/s.txt", "12345"))))
    report_file(base + "/s.txt")
    match Fs.metadata(base) {{
        Ok(m) -> println("dir_is_dir=" + to_string(Fs.meta_is_dir(m))),
        Err(_) -> println("dir_is_dir=ERR"),
    }}
}}

fn report_file(path: String) -> Unit with IO {{
    match Fs.metadata(path) {{
        Ok(m) -> do {{
            println("size=" + to_string(Fs.file_size(m)))
            println("is_file=" + to_string(Fs.meta_is_file(m)))
            println("is_dir=" + to_string(Fs.meta_is_dir(m)))
            println("agrees=" + to_string(Fs.meta_is_file(m) == Fs.is_file(path)))
            println("mtime=" + to_string(Fs.modified_time(m) > 0))
        }},
        Err(_) -> println("size=ERR"),
    }}
}}
"#
        ),
    );
    let _ = std::fs::remove_dir_all(workspace_root().join(base));
    assert!(success, "run failed:\n{stdout}\n{stderr}");
    for expected in [
        "size=5",
        "is_file=true",
        "is_dir=false",
        "agrees=true",
        "mtime=true",
        "dir_is_dir=true",
    ] {
        assert!(stdout.contains(expected), "missing {expected}:\n{stdout}");
    }
}

/// Statting a missing path is recoverable and names the path it tried.
#[test]
fn metadata_on_a_missing_path_is_recoverable() {
    let (stdout, stderr, success) = run_source(
        "fs_metadata_missing.flx",
        r#"
import Flow.Fs as Fs
import Flow.IoError as Io

fn main() -> Unit with IO {
    match Fs.metadata("/nonexistent/meta.txt") {
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
        stdout.contains("path=/nonexistent/meta.txt"),
        "got:\n{stdout}"
    );
    assert!(
        stdout.contains("still running"),
        "a failed stat must not abort:\n{stdout}"
    );
}

/// Inspection carries `FileSystem` like every other operation here.
#[test]
fn list_dir_requires_the_filesystem_effect() {
    let (stdout, stderr, success) = run_source(
        "fs_list_effect.flx",
        r#"
import Flow.Fs as Fs
import Flow.Result as Result

fn sneaky() -> Bool {
    Result.is_ok(Fs.list_dir("src"))
}

fn main() -> Unit with IO {
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
