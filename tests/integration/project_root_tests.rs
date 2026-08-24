//! Project roots come from `flux.toml`, and module search roots are computed
//! from the entry file rather than the process CWD.
//!
//! `collect_roots` previously looked for `src`/`lib` beside `Path::new(".")`,
//! so the resolved module roots depended on the directory `flux` was invoked
//! from: `flux run foo/bar.flx` found `src/` but `cd foo && flux run bar.flx`
//! did not. `find_project_root` also keyed only on `Cargo.toml`, so a Flux
//! project outside this checkout had no root at all and its build artifacts
//! landed beside the entry file.

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

fn flux_bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_flux"))
}

/// A two-directory project: `src/Helper.flx` beside a `flux.toml`, with the
/// entry file one level down in `foo/`. Importing `Helper` from `foo/bar.flx`
/// only succeeds if `src` is reachable from the *project*, not from the CWD.
fn write_project(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::create_dir_all(dir.join("foo")).expect("create foo");
    std::fs::write(
        dir.join("flux.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    )
    .expect("write flux.toml");
    std::fs::write(
        dir.join("src").join("Helper.flx"),
        "module Helper {\n    public fn greet() -> String { \"hi\" }\n}\n",
    )
    .expect("write Helper.flx");
    let entry = dir.join("foo").join("bar.flx");
    std::fs::write(
        &entry,
        "import Helper as Helper\n\nfn main() with IO { print(Helper.greet()) }\n",
    )
    .expect("write bar.flx");
    entry
}

fn run(entry: &Path, cwd: &Path) -> (String, bool) {
    let scratch = Scratch::new("project-root-run");
    let output = Command::new(flux_bin())
        .current_dir(cwd)
        .arg(entry)
        .arg("--no-cache")
        .args(scratch.cache_args())
        .env("NO_COLOR", "1")
        .output()
        .expect("run flux");
    let mut text = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    text.push_str(&String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"));
    (text, output.status.success())
}

/// The headline guarantee: the same program resolves the same modules whether
/// it is named relative to the project root or from inside a subdirectory.
#[test]
fn module_roots_do_not_depend_on_the_working_directory() {
    let scratch = Scratch::new("project-root-cwd");
    let project = scratch.path().join("proj");
    let entry = write_project(&project);

    // Named from the project root: `flux run foo/bar.flx`.
    let (from_root, root_ok) = run(Path::new("foo/bar.flx"), &project);
    // Named from inside the subdirectory: `cd foo && flux run bar.flx`.
    let (from_sub, sub_ok) = run(Path::new("bar.flx"), &project.join("foo"));
    // Named absolutely from an unrelated directory.
    let (from_away, away_ok) = run(&entry, scratch.path());

    assert!(
        root_ok,
        "running from the project root failed:\n{from_root}"
    );
    assert!(sub_ok, "running from the subdirectory failed:\n{from_sub}");
    assert!(
        away_ok,
        "running from outside the project failed:\n{from_away}"
    );
    for (label, out) in [
        ("project root", &from_root),
        ("subdirectory", &from_sub),
        ("outside", &from_away),
    ] {
        assert!(out.contains("hi"), "{label} did not resolve Helper:\n{out}");
    }
}

/// `flux.toml` marks the project root, so build artifacts land in
/// `<project>/target/flux` even outside this Rust checkout.
#[test]
fn flux_toml_marks_the_project_root_for_cache_placement() {
    let scratch = Scratch::new("project-root-cache");
    let project = scratch.path().join("proj");
    let entry = write_project(&project);

    // No `--cache-dir` here: the point is where flux chooses to put artifacts.
    let output = Command::new(flux_bin())
        .current_dir(&project)
        .arg(&entry)
        .env("NO_COLOR", "1")
        .output()
        .expect("run flux");
    assert!(
        output.status.success(),
        "run failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        project.join("target").join("flux").is_dir(),
        "expected artifacts under <project>/target/flux, found: {:?}",
        std::fs::read_dir(&project)
            .map(|entries| entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .collect::<Vec<_>>())
            .unwrap_or_default()
    );
}

/// Script mode must keep working: a loose `.flx` file with no manifest
/// anywhere above it still runs.
#[test]
fn manifest_less_script_mode_still_runs() {
    let scratch = Scratch::new("project-root-script");
    let entry = scratch.path().join("script.flx");
    std::fs::write(&entry, "fn main() with IO { print(1 + 1) }\n").expect("write script");

    let (out, ok) = run(&entry, scratch.path());
    assert!(ok, "script mode failed:\n{out}");
    assert!(out.contains('2'), "unexpected script output:\n{out}");
}
