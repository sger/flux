//! The C runtime's unit tests: C files in `runtime/c/tests/` that call the
//! runtime directly, built against it and linked the way a native Flux
//! program is (`runtime/c/tests/flux_test.h` says how to write one).
//!
//! `c_tests!` turns every C case into its own test per runtime variant, as
//! `c_runtime_tests::<file>::<case>::<variant>`, so a failure names its case
//! and `cargo test`/nextest can pick one out. Each file's `lists_every_case`
//! test fails when the file and the list here disagree.

#![cfg(feature = "llvm")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use flux::llvm::pipeline::{RuntimeVariant, build_c_program, ensure_runtime_lib, linked_exe_path};

fn runtime_c_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime").join("c")
}

fn variant_named(name: &str) -> RuntimeVariant {
    match name {
        "plain" => RuntimeVariant::Plain,
        "sanitize" => RuntimeVariant::Sanitize,
        "debug_heap" => RuntimeVariant::DebugHeap,
        other => panic!("unknown runtime variant `{other}`"),
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Held while a test binary is built: nextest runs each case in its own
/// process, and they share one binary per file and variant.
struct BuildLock(PathBuf);

impl BuildLock {
    fn acquire(path: PathBuf) -> Self {
        loop {
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return BuildLock(path),
                // Windows refuses to create a file another process is still
                // deleting, which is the lock being let go.
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
                    ) =>
                {
                    // A lock older than a build can take was left by a killed run.
                    let stale = modified(&path)
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > Duration::from_secs(300));
                    if stale {
                        let _ = fs::remove_file(&path);
                    } else {
                        std::thread::sleep(Duration::from_millis(25));
                    }
                }
                Err(err) => panic!("{}: {err}", path.display()),
            }
        }
    }
}

impl Drop for BuildLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// The test binary for `file` on `variant`, rebuilt only when the file,
/// `flux_test.h`, the runtime library or this test binary (which changes
/// with the Rust staticlib it links) is newer than it.
fn test_binary(file: &str, variant: RuntimeVariant) -> PathBuf {
    let dir = runtime_c_dir();
    let source = dir.join("tests").join(file);
    let out_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("c_tests");
    fs::create_dir_all(&out_dir).expect("create the C test directory");
    let stem = format!("{}_{}", file.trim_end_matches(".c"), variant.stem());
    let exe = out_dir.join(&stem);
    let linked = linked_exe_path(&exe);

    let _lock = BuildLock::acquire(out_dir.join(format!("{stem}.lock")));
    ensure_runtime_lib(&dir, variant).unwrap_or_else(|err| panic!("runtime library: {err:?}"));
    let inputs = [
        source.clone(),
        dir.join("tests").join("flux_test.h"),
        dir.join(variant.file_name()),
        std::env::current_exe().expect("test binary path"),
    ];
    let fresh = modified(&linked)
        .is_some_and(|built| inputs.iter().all(|i| modified(i).is_some_and(|t| t <= built)));
    if !fresh {
        build_c_program(&[source], &dir, variant, &exe)
            .unwrap_or_else(|err| panic!("building {file}: {err:?}"));
    }
    linked
}

fn run_test_binary(file: &str, variant: RuntimeVariant, env: &[(&str, &str)]) -> Output {
    let exe = test_binary(file, variant);
    let mut command = Command::new(&exe);
    command.envs(env.iter().copied());
    if cfg!(windows) && variant == RuntimeVariant::Sanitize {
        // clang links AddressSanitizer's runtime as a DLL on Windows, from
        // its resource directory, which is not on PATH.
        let resource_dir = Command::new("clang")
            .arg("-print-resource-dir")
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
            .unwrap_or_default();
        let dll_dir = Path::new(&resource_dir).join("lib").join("windows");
        let path = std::env::var_os("PATH").unwrap_or_default();
        let joined =
            std::env::join_paths(std::iter::once(dll_dir).chain(std::env::split_paths(&path)))
                .expect("PATH entries join");
        command.env("PATH", joined);
    }
    command.output().unwrap_or_else(|err| panic!("running {}: {err}", exe.display()))
}

/// Runs one case of `file` on the runtime variant named `variant`.
fn run_case(file: &str, case: &str, variant: &str) {
    let output = run_test_binary(file, variant_named(variant), &[("FLUX_TEST_CASE", case)]);
    assert!(
        output.status.success(),
        "{file}: case {case} failed on the {variant} runtime ({}):\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The cases `file` defines must be the ones listed for it.
fn check_case_list(file: &str, listed: &[&str]) {
    let output = run_test_binary(file, RuntimeVariant::Plain, &[("FLUX_TEST_LIST", "1")]);
    assert!(output.status.success(), "{file}: listing its cases failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut defined: Vec<&str> = stdout.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let mut listed = listed.to_vec();
    defined.sort_unstable();
    listed.sort_unstable();
    assert_eq!(defined, listed, "{file}: the cases it defines and the ones c_tests! lists differ");
}

/// `c_tests!(module, "file.c", [variants], [cases])`: one test per case and
/// variant, `module::case::variant`, plus `module::lists_every_case`.
macro_rules! c_tests {
    ($module:ident, $file:literal, $variants:tt, [$($case:ident),+ $(,)?]) => {
        mod $module {
            #[test]
            fn lists_every_case() {
                super::check_case_list($file, &[$(stringify!($case)),+]);
            }
            $( c_tests!(@case $file, $case, $variants); )+
        }
    };
    (@case $file:literal, $case:ident, [$($variant:ident),+]) => {
        mod $case {
            $(
                #[test]
                fn $variant() {
                    super::super::run_case($file, stringify!($case), stringify!($variant));
                }
            )+
        }
    };
}

c_tests!(
    smoke,
    "smoke_test.c",
    [plain],
    [
        tag_int,
        tag_bool,
        tag_none,
        tag_ptr,
        gc_alloc,
        string_new,
        string_concat,
        string_slice,
        string_int_conversions,
        string_eq,
        hamt_set_get_contains,
        hamt_many_keys_and_delete,
        drop_deep_list,
        drop_shared_list_keeps_it,
        drop_deep_tree,
        drop_deep_single_field_chain,
    ]
);

// The sanitized runtime stops at the first signed overflow, which C leaves
// undefined even where the hardware wraps.
c_tests!(
    int_arith_ki219,
    "int_arith_test.c",
    [plain, sanitize],
    [
        add_wraps,
        sub_wraps,
        mul_wraps,
        neg_and_abs_wrap,
        div_by_minus_one_negates,
        mod_by_minus_one_is_zero,
    ]
);
