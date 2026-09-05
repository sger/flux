//! End-to-end coverage for the registry-independent part of proposal 0177.
//!
//! These tests use temporary local `file://` repositories, so they exercise
//! the real git subprocess, checkout store, lockfile, and package CLI without
//! depending on a registry or the network.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

fn flux_bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_flux"))
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed:\n{}{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn commit(repo: &Path, message: &str) -> String {
    git(repo, &["add", "."]);
    git(repo, &["commit", "-m", message]);
    let output = Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("read git commit");
    assert!(output.status.success(), "rev-parse failed");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn new_git_package(scratch: &Scratch) -> (PathBuf, String) {
    let repo = scratch.join("shared-repo");
    std::fs::create_dir_all(&repo).expect("create git repository");
    git(&repo, &["init"]);
    git(&repo, &["config", "user.email", "tests@flux-lang.org"]);
    git(&repo, &["config", "user.name", "Flux tests"]);
    git(&repo, &["checkout", "-b", "main"]);
    std::fs::write(
        repo.join("flux.toml"),
        "[package]\nname = \"shared\"\nversion = \"0.1.0\"\n",
    )
    .expect("write dependency manifest");
    std::fs::create_dir_all(repo.join("src")).expect("create dependency source dir");
    std::fs::write(
        repo.join("src/Shared.flx"),
        "module Shared { public fn tag() -> String { \"shared v1\" } }\n",
    )
    .expect("write dependency source");
    let first = commit(&repo, "shared v1");
    (repo, first)
}

/// A `file://` URL for `path`.
///
/// The separators are rewritten because the URL is written into a
/// `flux.toml` string, where a Windows path's `\C` is an invalid escape
/// and the manifest fails to parse before git is ever reached.
fn file_url(path: &Path) -> String {
    format!("file://{}", path.to_string_lossy().replace('\\', "/"))
}

fn app(scratch: &Scratch, dependency: &str) -> PathBuf {
    let app = scratch.join("app");
    std::fs::create_dir_all(app.join("src")).expect("create app");
    std::fs::write(
        app.join("flux.toml"),
        format!(
            "# Keep this comment while `flux add` edits the dependency.\n\
             [package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
             [dependencies]\n{dependency}\n"
        ),
    )
    .expect("write app manifest");
    std::fs::write(
        app.join("src/main.flx"),
        "import Shared as Shared\n\nfn main() with IO { print(Shared.tag()) }\n",
    )
    .expect("write app source");
    app
}

fn flux(args: &[String], cwd: &Path, scratch: &Scratch) -> Output {
    Command::new(flux_bin())
        .current_dir(cwd)
        .args(args)
        .args(scratch.cache_args())
        .env("FLUX_HOME", scratch.join("flux-home"))
        .env("NO_COLOR", "1")
        .output()
        .expect("run flux")
}

fn text(output: &Output) -> String {
    let mut combined = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    combined.push_str(&String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"));
    combined
}

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

#[test]
fn git_dependency_builds_and_records_its_commit() {
    let scratch = Scratch::new("pkg-git-build");
    let (repo, commit) = new_git_package(&scratch);
    let url = file_url(&repo);
    let app = app(
        &scratch,
        &format!("shared = {{ git = \"{url}\", rev = \"{commit}\" }}"),
    );

    let output = flux(&args(&["run"]), &app, &scratch);
    assert!(
        output.status.success(),
        "git build failed:\n{}",
        text(&output)
    );
    assert!(
        text(&output).contains("shared v1"),
        "unexpected output:\n{}",
        text(&output)
    );

    let lock = std::fs::read_to_string(app.join("flux.lock")).expect("read lockfile");
    assert!(
        lock.contains(&format!("source = \"git+{url}#{commit}\"")),
        "{lock}"
    );
}

#[test]
fn offline_build_uses_the_locked_git_checkout() {
    let scratch = Scratch::new("pkg-git-offline");
    let (repo, commit) = new_git_package(&scratch);
    let url = file_url(&repo);
    let app = app(
        &scratch,
        &format!("shared = {{ git = \"{url}\", rev = \"{commit}\" }}"),
    );

    let first = flux(&args(&["run"]), &app, &scratch);
    assert!(
        first.status.success(),
        "initial build failed:\n{}",
        text(&first)
    );
    std::fs::remove_dir_all(&repo).expect("remove source repository");

    let offline = flux(&args(&["run", "--offline"]), &app, &scratch);
    assert!(
        offline.status.success(),
        "offline build did not reuse the checkout:\n{}",
        text(&offline)
    );
    assert!(
        text(&offline).contains("shared v1"),
        "unexpected output:\n{}",
        text(&offline)
    );
}

#[test]
fn add_remove_and_tree_cover_a_git_dependency() {
    let scratch = Scratch::new("pkg-git-cli");
    let (repo, commit) = new_git_package(&scratch);
    let url = file_url(&repo);
    let app = app(&scratch, "");

    let added = flux(
        &args(&["add", "shared", "--git", &url, "--rev", &commit]),
        &app,
        &scratch,
    );
    assert!(added.status.success(), "flux add failed:\n{}", text(&added));
    let manifest = std::fs::read_to_string(app.join("flux.toml")).expect("read manifest");
    assert!(manifest.starts_with("# Keep this comment"), "{manifest}");
    assert!(manifest.contains(&format!(
        "shared = {{ git = \"{url}\", rev = \"{commit}\" }}"
    )));

    let built = flux(&args(&["run"]), &app, &scratch);
    assert!(
        built.status.success(),
        "git dependency failed to build:\n{}",
        text(&built)
    );

    let tree = flux(&args(&["tree"]), &app, &scratch);
    assert!(tree.status.success(), "flux tree failed:\n{}", text(&tree));
    assert!(
        text(&tree).contains("shared"),
        "tree omitted dependency:\n{}",
        text(&tree)
    );
    assert!(
        text(&tree).contains(&commit[..7]),
        "tree omitted locked commit:\n{}",
        text(&tree)
    );

    let removed = flux(&args(&["remove", "shared"]), &app, &scratch);
    assert!(
        removed.status.success(),
        "flux remove failed:\n{}",
        text(&removed)
    );
    let manifest = std::fs::read_to_string(app.join("flux.toml")).expect("read manifest");
    assert!(manifest.starts_with("# Keep this comment"), "{manifest}");
    assert!(
        !manifest.contains("shared ="),
        "dependency was not removed:\n{manifest}"
    );
}

#[test]
fn update_refreshes_a_branch_and_reports_the_new_commit() {
    let scratch = Scratch::new("pkg-git-update");
    let (repo, first) = new_git_package(&scratch);
    let url = file_url(&repo);
    let app = app(
        &scratch,
        &format!("shared = {{ git = \"{url}\", branch = \"main\" }}"),
    );

    let initial = flux(&args(&["run"]), &app, &scratch);
    assert!(
        initial.status.success(),
        "initial branch build failed:\n{}",
        text(&initial)
    );

    std::fs::write(
        repo.join("src/Shared.flx"),
        "module Shared { public fn tag() -> String { \"shared v2\" } }\n",
    )
    .expect("update dependency source");
    let second = commit(&repo, "shared v2");

    let updated = flux(&args(&["update"]), &app, &scratch);
    assert!(
        updated.status.success(),
        "flux update failed:\n{}",
        text(&updated)
    );
    assert!(
        text(&updated).contains(&format!("{} -> {}", &first[..7], &second[..7])),
        "update did not report the commit change:\n{}",
        text(&updated)
    );

    let lock = std::fs::read_to_string(app.join("flux.lock")).expect("read updated lockfile");
    assert!(
        lock.contains(&format!("#{second}")),
        "lockfile was not updated:\n{lock}"
    );
    let rebuilt = flux(&args(&["run"]), &app, &scratch);
    assert!(
        rebuilt.status.success(),
        "updated build failed:\n{}",
        text(&rebuilt)
    );
    assert!(
        text(&rebuilt).contains("shared v2"),
        "updated source was not built:\n{}",
        text(&rebuilt)
    );
}
