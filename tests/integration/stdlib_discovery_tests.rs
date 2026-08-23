//! Flux must find its stdlib when run from outside the checkout (KI-008).
//!
//! `inject_flow_prelude` resolved `lib/Flow` against the process CWD and
//! returned silently when it was missing, and `collect_roots` did the same for
//! `src`/`lib`. Running anywhere else produced "Looked for module `Flow.List`
//! under roots: " — an empty root list rather than a diagnosis. That blocked
//! installing Flux as a tool.
//!
//! Resolution now tries `$FLUX_LIB_DIR`, then `lib/Flow` walking up from the
//! entry file, then `lib/Flow` walking up from the executable.

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

const SOURCE: &str = r#"
import Flow.List as List

fn main() with IO {
    print(to_string(len(List.map([1, 2, 3], \x -> x + 1))))
}
"#;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Run the entry file with `cwd` as the working directory. `flux_bin` lets a
/// test point at a copy of the binary in an installed-style tree.
fn run(flux_bin: &Path, entry: &Path, cwd: &Path, env: &[(&str, &str)]) -> (String, String, bool) {
    let scratch = Scratch::new("stdlib-discovery-run");
    let mut cmd = Command::new(flux_bin);
    cmd.current_dir(cwd)
        .arg(entry)
        .arg("--no-cache")
        .args(scratch.cache_args())
        .env("NO_COLOR", "1");
    for (key, value) in env {
        cmd.env(key, value);
    }
    let output = cmd.output().expect("run flux");
    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

fn flux_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_flux"))
}

#[test]
fn a_program_outside_the_checkout_finds_the_stdlib() {
    let scratch = Scratch::new("stdlib-discovery-outside");
    let entry = scratch.write("uses_stdlib.flx", SOURCE);

    // cwd is the scratch dir, which has no `lib/Flow` above it inside the
    // repo — the discovery must come from the executable's location.
    let (stdout, stderr, ok) = run(&flux_bin(), &entry, scratch.path(), &[]);
    assert!(ok, "run failed:\n{stdout}{stderr}");
    assert!(
        stdout.contains('3'),
        "expected the mapped list length:\n{stdout}{stderr}"
    );
}

#[test]
fn flux_lib_dir_overrides_discovery() {
    let scratch = Scratch::new("stdlib-discovery-env");
    let entry = scratch.write("uses_stdlib.flx", SOURCE);
    let lib = workspace_root().join("lib");

    let (stdout, stderr, ok) = run(
        &flux_bin(),
        &entry,
        scratch.path(),
        &[("FLUX_LIB_DIR", lib.to_str().unwrap())],
    );
    assert!(ok, "run failed:\n{stdout}{stderr}");
    assert!(stdout.contains('3'), "unexpected output:\n{stdout}{stderr}");
}

#[test]
fn an_unusable_flux_lib_dir_falls_through_to_the_other_candidates() {
    let scratch = Scratch::new("stdlib-discovery-badenv");
    let entry = scratch.write("uses_stdlib.flx", SOURCE);

    // A bad override must not be fatal: the remaining candidates still apply.
    let (stdout, stderr, ok) = run(
        &flux_bin(),
        &entry,
        scratch.path(),
        &[("FLUX_LIB_DIR", "/nonexistent/flux/lib")],
    );
    assert!(ok, "run failed:\n{stdout}{stderr}");
    assert!(stdout.contains('3'), "unexpected output:\n{stdout}{stderr}");
}

/// The case KI-008 called out: an installed tree, `<prefix>/bin/flux` with
/// `<prefix>/lib/Flow` beside it, invoked from an unrelated directory.
#[test]
fn an_installed_binary_finds_its_stdlib() {
    let scratch = Scratch::new("stdlib-discovery-installed");
    let entry = scratch.write("uses_stdlib.flx", SOURCE);

    let prefix = scratch.join("prefix");
    let bin_dir = prefix.join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create bin dir");
    let installed = bin_dir.join("flux");
    std::fs::copy(flux_bin(), &installed).expect("copy flux binary");

    copy_dir(
        &workspace_root().join("lib").join("Flow"),
        &prefix.join("lib").join("Flow"),
    );

    let (stdout, stderr, ok) = run(&installed, &entry, scratch.path(), &[]);
    assert!(ok, "installed binary failed:\n{stdout}{stderr}");
    assert!(stdout.contains('3'), "unexpected output:\n{stdout}{stderr}");
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create dir");
    for entry in std::fs::read_dir(from).expect("read dir") {
        let entry = entry.expect("dir entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}
