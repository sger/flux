#![cfg(feature = "jit")]

mod diagnostics_env;
#[path = "support/examples_snapshot.rs"]
mod examples_snapshot;

use std::path::Path;
use std::process::Command;

fn normalize_cli_text(text: &str, workspace_root: &Path) -> String {
    let normalized = examples_snapshot::normalize_transcript(text, workspace_root);
    let lines = normalized
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    lines.trim().to_string()
}

fn run_flux_file(
    workspace_root: &Path,
    flux_bin: &Path,
    file: &str,
    jit: bool,
) -> (i32, String, String) {
    let mut args = vec!["--no-cache".to_string(), file.to_string()];
    if jit {
        args.push("--jit".to_string());
    }

    let output = Command::new(flux_bin)
        .current_dir(workspace_root)
        .args(&args)
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux for `{file}` (jit={jit}): {e}"));

    let status = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (status, stdout, stderr)
}

fn build_runtime_transcript(workspace_root: &Path, flux_bin: &Path, fixture_rel: &str) -> String {
    let (vm_status, vm_stdout, vm_stderr) =
        run_flux_file(workspace_root, flux_bin, fixture_rel, false);
    let (jit_status, jit_stdout, jit_stderr) =
        run_flux_file(workspace_root, flux_bin, fixture_rel, true);

    format!(
        "Fixture: {fixture_rel}\n== vm ==\nstatus: {vm_status}\nstdout:\n{}\nstderr:\n{}\n== jit ==\nstatus: {jit_status}\nstdout:\n{}\nstderr:\n{}\n",
        if vm_stdout.trim().is_empty() {
            String::from("<empty>")
        } else {
            normalize_cli_text(&vm_stdout, workspace_root)
        },
        if vm_stderr.trim().is_empty() {
            String::from("<empty>")
        } else {
            normalize_cli_text(&vm_stderr, workspace_root)
        },
        if jit_stdout.trim().is_empty() {
            String::from("<empty>")
        } else {
            normalize_cli_text(&jit_stdout, workspace_root)
        },
        if jit_stderr.trim().is_empty() {
            String::from("<empty>")
        } else {
            normalize_cli_text(&jit_stderr, workspace_root)
        },
    )
}

#[test]
fn runtime_error_fixtures_snapshot() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let flux_bin = Path::new(env!("CARGO_BIN_EXE_flux"));
    let fixtures_root = workspace_root.join("examples/runtime_errors");
    let fixtures = examples_snapshot::discover_fixtures(&fixtures_root);

    assert!(
        !fixtures.is_empty(),
        "no .flx fixtures found under `{}`",
        fixtures_root.display()
    );

    for fixture in fixtures
        .into_iter()
        .filter(|fixture| !fixture.to_string_lossy().contains("/RuntimeErrors/"))
    {
        let rel = fixture
            .strip_prefix(workspace_root)
            .unwrap_or(&fixture)
            .to_string_lossy()
            .replace('\\', "/");
        let snapshot_name = examples_snapshot::snapshot_name(&fixtures_root, &fixture);
        let transcript = build_runtime_transcript(workspace_root, flux_bin, &rel);

        insta::with_settings!({
            snapshot_path => "snapshots/runtime_error_fixtures",
            prepend_module_to_snapshot => false,
            omit_expression => true,
        }, {
            insta::assert_snapshot!(snapshot_name, transcript);
        });
    }
}

#[test]
fn runtime_boundary_errors_do_not_double_wrap_e1004_in_vm() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let flux_bin = Path::new(env!("CARGO_BIN_EXE_flux"));
    let (_status, _stdout, vm_stderr) = run_flux_file(
        workspace_root,
        flux_bin,
        "examples/runtime_errors/boundary_arg_string_into_int.flx",
        false,
    );

    assert!(
        vm_stderr.contains("Error[E1004]: Type Error"),
        "expected VM stderr to render E1004 directly, got:\n{vm_stderr}"
    );
    assert!(
        !vm_stderr.contains("E1009"),
        "expected VM stderr not to double-wrap E1004, got:\n{vm_stderr}"
    );
}
