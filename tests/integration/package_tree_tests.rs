//! `flux tree` renders the resolved dependency graph.
//!
//! The drawing is the whole value of the command, so these tests snapshot it
//! rather than asserting that a name appears somewhere in the output: the
//! connectors, the indentation of a nested level, and the `└──` on the last
//! child are what a reader relies on and what a refactor is most likely to
//! break silently.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::{Scratch, cache_args_for};

fn flux(args: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(cwd)
        .args(args)
        .args(cache_args_for(&cwd.join("target").join("flux")))
        .env("NO_COLOR", "1")
        .output()
        .expect("run flux")
}

fn text(output: &Output) -> String {
    let mut s = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    s.push_str(&String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"));
    s.trim_end().to_string()
}

/// Write a package with the given name, version and `[dependencies]` body.
fn package(root: &Path, name: &str, version: &str, deps: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(dir.join("src")).expect("create package dir");
    let manifest = format!(
        "[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2026\"\n{deps}"
    );
    std::fs::write(dir.join("flux.toml"), manifest).expect("write manifest");
    std::fs::write(
        dir.join("src").join("main.flx"),
        "fn main() with IO { print(\"hi\") }\n",
    )
    .expect("write source");
    dir
}

#[test]
fn tree_of_a_package_without_dependencies() {
    let scratch = Scratch::new("tree-leaf");
    let app = package(scratch.path(), "app", "0.1.0", "");

    let out = flux(&["tree"], &app);
    assert!(out.status.success(), "tree failed:\n{}", text(&out));
    insta::assert_snapshot!(text(&out));
}

/// Two path dependencies, one of which has a dependency of its own: covers the
/// `├──`/`└──` choice and the indentation carried into a nested level.
#[test]
fn tree_of_a_nested_dependency_graph() {
    let scratch = Scratch::new("tree-nested");
    let root = scratch.path();

    package(root, "leaf", "0.3.0", "");
    package(
        root,
        "mid",
        "0.2.0",
        "\n[dependencies]\nleaf = { path = \"../leaf\" }\n",
    );
    let app = package(
        root,
        "app",
        "0.1.0",
        "\n[dependencies]\nmid = { path = \"../mid\" }\nleaf = { path = \"../leaf\" }\n",
    );

    let out = flux(&["tree"], &app);
    assert!(out.status.success(), "tree failed:\n{}", text(&out));
    insta::assert_snapshot!(text(&out));
}
