mod diagnostics_env;

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn example_path(rel: &str) -> PathBuf {
    workspace_root().join("examples").join(rel)
}

fn run_flux_output(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux with args {:?}: {e}", args))
}

fn run_flux(args: &[&str]) -> String {
    let output = run_flux_output(args);
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    normalize_transcript(&text.replace("\r\n", "\n"))
}

fn run_flux_trace(args: &[&str]) -> (String, String) {
    let output = run_flux_output(args);
    assert!(
        output.status.success(),
        "expected flux {:?} to succeed, status={:?}\nstdout:\n{}\nstderr:\n{}",
        args,
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout =
        normalize_transcript(&String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"));
    let stderr =
        normalize_transcript(&String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"));
    (stdout, stderr)
}

fn run_flux_trace_snapshot(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux with args {:?}: {e}", args));

    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    normalize_transcript(&text.replace("\r\n", "\n"))
}

fn normalize_transcript(text: &str) -> String {
    let ws = workspace_root().to_string_lossy().to_string();
    let ws_forward = ws.replace('\\', "/");
    let mut normalized = Vec::new();
    let mut pending_drops = Vec::new();

    for line in text.lines() {
        // Replace absolute workspace path with a stable placeholder so
        // snapshots don't break across machines (local vs CI).
        let mut line = line
            .replace(&ws, "<workspace>")
            .replace(&ws_forward, "<workspace>")
            .replace("<workspace>\\", "<workspace>/");
        if line.contains("<workspace>") {
            line = line.replace('\\', "/");
        }

        if is_plain_drop_line(&line) {
            pending_drops.push(line);
            continue;
        }

        flush_sorted_drops(&mut normalized, &mut pending_drops);
        normalized.push(line);
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

fn assert_trace_snapshot(rel: &str, args: &[&str], mode: &str) {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let file = example_path(rel);
    let mut cmd = args.to_vec();
    cmd.push(file.to_str().unwrap());
    let transcript = run_flux_trace_snapshot(&cmd);
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
fn snapshot_bench_reuse_enabled_dump_aether() {
    assert_cli_snapshot(
        "aether/bench_reuse_enabled.flx",
        &["--dump-aether"],
        "dump_aether",
    );
}

#[test]
fn snapshot_bench_reuse_blocked_dump_aether() {
    assert_cli_snapshot(
        "aether/bench_reuse_blocked.flx",
        &["--dump-aether"],
        "dump_aether",
    );
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
fn snapshot_queue_workload_dump_core_debug() {
    assert_cli_snapshot(
        "aether/queue_workload.flx",
        &["--dump-core=debug"],
        "dump_core_debug",
    );
}

#[test]
fn snapshot_forwarded_wrapper_reuse_dump_aether() {
    assert_cli_snapshot(
        "aether/forwarded_wrapper_reuse.flx",
        &["--dump-aether"],
        "dump_aether",
    );
}

#[test]
fn snapshot_opt_corpus_positive_dump_aether() {
    assert_cli_snapshot(
        "aether/opt_corpus_positive.flx",
        &["--dump-aether"],
        "dump_aether",
    );
}

#[test]
fn snapshot_opt_corpus_negative_dump_aether() {
    assert_cli_snapshot(
        "aether/opt_corpus_negative.flx",
        &["--dump-aether"],
        "dump_aether",
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

#[test]
fn snapshot_verify_aether_trace_aether_vm() {
    assert_trace_snapshot(
        "aether/verify_aether.flx",
        &["--trace-aether"],
        "trace_aether_vm",
    );
}

#[test]
fn trace_aether_emits_report_on_stderr_and_program_output_on_stdout() {
    let file = example_path("aether/verify_aether.flx");
    let (stdout, stderr) = run_flux_trace(&["--trace-aether", file.to_str().unwrap()]);
    assert!(
        stderr.contains("── Aether Trace ──"),
        "stderr was:\n{stderr}"
    );
    assert!(
        stderr.contains("Aether Memory Model Report"),
        "stderr was:\n{stderr}"
    );
    assert!(stderr.contains("backend: vm"), "stderr was:\n{stderr}");
    assert!(
        stderr.contains("pipeline: AST -> Core -> CFG -> bytecode -> VM"),
        "stderr was:\n{stderr}"
    );
    assert!(!stdout.trim().is_empty(), "stdout was empty");
    assert!(
        stdout.contains("[2, 4, 6]"),
        "stdout should contain the program output; stdout was:\n{stdout}"
    );
}

#[test]
fn dump_aether_and_trace_aether_share_report_content() {
    let file = example_path("aether/verify_aether.flx");

    let dump = run_flux(&["--dump-aether", file.to_str().unwrap()]);
    let (_stdout, stderr) = run_flux_trace(&["--trace-aether", file.to_str().unwrap()]);
    let report = dump
        .split_once("Aether Memory Model Report")
        .map(|(_, rest)| format!("Aether Memory Model Report{rest}"))
        .unwrap_or(dump);
    let report_only = report
        .split("\nWarning:")
        .next()
        .unwrap_or(report.as_str())
        .trim_end();

    assert!(
        stderr.contains(report_only),
        "trace stderr should include the dump-aether report body\n== report ==\n{report_only}\n== stderr ==\n{stderr}"
    );
}
