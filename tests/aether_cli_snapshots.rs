mod diagnostics_env;

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn example_path(rel: &str) -> PathBuf {
    workspace_root().join("examples").join(rel)
}

fn run_flux(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux with args {:?}: {e}", args));

    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    normalize_transcript(&text.replace("\r\n", "\n"))
}

fn normalize_transcript(text: &str) -> String {
    let mut normalized = Vec::new();
    let mut pending_drops = Vec::new();

    for line in text.lines() {
        if is_plain_drop_line(line) {
            pending_drops.push(line.to_string());
            continue;
        }

        flush_sorted_drops(&mut normalized, &mut pending_drops);
        normalized.push(line.to_string());
    }

    flush_sorted_drops(&mut normalized, &mut pending_drops);
    normalized.join("\n")
}

fn flush_sorted_drops(normalized: &mut Vec<String>, pending_drops: &mut Vec<String>) {
    pending_drops.sort();
    normalized.append(pending_drops);
}

fn is_plain_drop_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("drop ") && !trimmed.contains('(')
}

fn snapshot_name(rel: &str, mode: &str) -> String {
    format!(
        "aether__{}__{}",
        rel.strip_suffix(".flx").unwrap_or(rel).replace('/', "__"),
        mode
    )
}

fn assert_cli_snapshot(rel: &str, args: &[&str], mode: &str) {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let file = example_path(rel);
    let mut cmd = args.to_vec();
    cmd.push(file.to_str().unwrap());
    let transcript = run_flux(&cmd);
    insta::with_settings!({
        snapshot_path => "snapshots/aether",
        prepend_module_to_snapshot => false,
        omit_expression => true,
    }, {
        insta::assert_snapshot!(snapshot_name(rel, mode), transcript);
    });
}

#[test]
fn snapshot_verify_aether_dump_core() {
    assert_cli_snapshot("aether/verify_aether.flx", &["--dump-core"], "dump_core");
}

#[test]
fn snapshot_verify_aether_dump_core_debug() {
    assert_cli_snapshot(
        "aether/verify_aether.flx",
        &["--dump-core=debug"],
        "dump_core_debug",
    );
}

#[test]
fn snapshot_verify_aether_dump_aether() {
    assert_cli_snapshot(
        "aether/verify_aether.flx",
        &["--dump-aether"],
        "dump_aether",
    );
}

#[test]
fn snapshot_bench_reuse_dump_aether() {
    assert_cli_snapshot("aether/bench_reuse.flx", &["--dump-aether"], "dump_aether");
}

#[test]
fn snapshot_hof_recursive_suite_dump_aether() {
    assert_cli_snapshot(
        "aether/hof_recursive_suite.flx",
        &["--dump-aether"],
        "dump_aether",
    );
}

#[test]
fn snapshot_tree_updates_dump_core_debug() {
    assert_cli_snapshot(
        "aether/tree_updates.flx",
        &["--dump-core=debug"],
        "dump_core_debug",
    );
}

#[test]
fn snapshot_fbip_success_cases_dump_aether() {
    assert_cli_snapshot(
        "aether/fbip_success_cases.flx",
        &["--dump-aether"],
        "dump_aether",
    );
}

#[test]
fn snapshot_fbip_failure_cases_dump_aether() {
    assert_cli_snapshot(
        "aether/fbip_failure_cases.flx",
        &["--dump-aether"],
        "dump_aether",
    );
}

#[test]
fn snapshot_borrow_calls_dump_core() {
    assert_cli_snapshot("aether/borrow_calls.flx", &["--dump-core"], "dump_core");
}

#[test]
fn snapshot_reuse_specialization_dump_core_debug() {
    assert_cli_snapshot(
        "aether/reuse_specialization.flx",
        &["--dump-core=debug"],
        "dump_core_debug",
    );
}

#[test]
fn snapshot_drop_spec_branchy_dump_core_debug() {
    assert_cli_snapshot(
        "aether/drop_spec_branchy.flx",
        &["--dump-core=debug"],
        "dump_core_debug",
    );
}

#[test]
fn snapshot_drop_spec_recursive_dump_core_debug() {
    assert_cli_snapshot(
        "aether/drop_spec_recursive.flx",
        &["--dump-core=debug"],
        "dump_core_debug",
    );
}

#[test]
fn snapshot_fbip_failure_dump_aether() {
    assert_cli_snapshot(
        "aether/fbip_fail_nonfip_call.flx",
        &["--dump-aether"],
        "dump_aether",
    );
}
