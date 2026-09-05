//! Package store, publish, and workspace tests.

use std::{path::Path, process::Command};

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

fn flux_bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_flux"))
}

fn run(args: &[&str], cwd: &Path, scratch: &Scratch) -> std::process::Output {
    Command::new(flux_bin())
        .current_dir(cwd)
        .args(args)
        .args(scratch.cache_args())
        .env("FLUX_HOME", scratch.join("flux-home"))
        .env("NO_COLOR", "1")
        .output()
        .expect("run flux")
}

fn text(output: &std::process::Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

fn package(dir: &Path, name: &str, body: &str) {
    std::fs::create_dir_all(dir.join("src")).expect("source dir");
    std::fs::write(
        dir.join("flux.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
    )
    .expect("manifest");
    std::fs::write(dir.join("src/main.flx"), body).expect("source");
}

#[test]
fn metadata_is_versioned_json() {
    let scratch = Scratch::new("pkg-phase3-metadata");
    let project = scratch.join("app");
    package(&project, "app", "fn main() with IO { print(\"ok\") }\n");
    let build = run(&["run"], &project, &scratch);
    assert!(build.status.success(), "{}", text(&build));
    assert!(
        scratch
            .join("flux-home/store")
            .join("flux-fxmc-26")
            .is_dir()
    );
    let output = run(&["metadata", "--format", "json"], &project, &scratch);
    assert!(output.status.success(), "{}", text(&output));
    let value: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).expect("metadata JSON");
    assert_eq!(value["format_version"], 1);
    assert_eq!(
        value["workspace"]["root"],
        project.to_string_lossy().as_ref()
    );
    assert!(
        value["packages"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
}

#[test]
fn publish_dry_run_verifies_a_clean_checkout_and_excludes_target() {
    let scratch = Scratch::new("pkg-phase3-publish");
    let project = scratch.join("app");
    package(&project, "app", "fn main() with IO { print(\"ok\") }\n");
    std::fs::create_dir_all(project.join("target")).expect("target");
    std::fs::write(project.join("target/should-not-ship"), "cache").expect("marker");
    let output = run(&["publish", "--dry-run"], &project, &scratch);
    assert!(output.status.success(), "{}", text(&output));
    let archive = project.join("target/flux/publish/app-0.1.0.tar");
    assert!(archive.is_file());
    let listing = Command::new("tar")
        .args(["-tf", archive.to_str().unwrap()])
        .output()
        .expect("tar list");
    let listing = String::from_utf8_lossy(&listing.stdout);
    assert!(!listing.contains("target/"));
    assert!(text(&output).contains("verified clean checkout"));
}

#[test]
fn workspace_member_metadata_uses_the_workspace_root() {
    let scratch = Scratch::new("pkg-phase3-workspace");
    let root = scratch.join("workspace");
    std::fs::create_dir_all(&root).expect("workspace");
    std::fs::write(
        root.join("flux.toml"),
        "[workspace]\nmembers = [\"member\"]\n",
    )
    .expect("workspace manifest");
    let member = root.join("member");
    package(
        &member,
        "member",
        "fn main() with IO { print(\"member\") }\n",
    );
    let output = run(&["metadata", "--format", "json"], &member, &scratch);
    assert!(output.status.success(), "{}", text(&output));
    let value: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).expect("metadata JSON");
    assert_eq!(value["workspace"]["root"], root.to_string_lossy().as_ref());
    assert_eq!(
        value["targets"]["cache_root"],
        root.join("target").join("flux").to_string_lossy().as_ref()
    );
    assert!(
        value["packages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["name"] == "member")
    );
}

#[test]
fn flux_store_module_keeps_unit_keys_path_independent() {
    let scratch = Scratch::new("pkg-phase3-store-module");
    let source = scratch.write(
        "store.flx",
        "import Flume.Build.Store as Store\n\nfn main() with IO { print(Store.unit_key(\"pkg\", \"src/Lib.flx\", \"source\", [], \"0.0.6\", Store.compiler_abi(), \"vm\", [\"strict=false\"])) }\n",
    );
    let output = Command::new(flux_bin())
        .current_dir(scratch.path())
        .args(["run", source.to_str().unwrap(), "--no-cache"])
        .args(scratch.cache_args())
        .output()
        .expect("run flux");
    assert!(output.status.success(), "{}", text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let digest = stdout.trim().trim_matches('"');
    assert_eq!(digest.len(), 64);
}
