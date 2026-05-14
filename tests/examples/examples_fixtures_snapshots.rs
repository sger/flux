#[path = "../support/diagnostics_env.rs"]
mod diagnostics_env;
#[path = "../support/examples_snapshot.rs"]
mod examples_snapshot;

use std::path::Path;

// Each function covers one top-level examples/ subdirectory so that
// `cargo test -j` can run them in parallel instead of compiling every
// fixture sequentially inside a single test function.

fn snapshot_dir(workspace_root: &Path, subdir: &str) -> Vec<examples_snapshot::FixtureSnapshotCase> {
    examples_snapshot::run_fixture_subdir_snapshots(workspace_root, "examples", subdir)
        .unwrap_or_else(|e| panic!("{e}"))
}

fn is_excluded(name: &str) -> bool {
    name.contains("max_errors")
        || name.starts_with("parser_errors__")
        || name.starts_with("compiler_errors__")
        || name.starts_with("runtime_errors__")
        || name.starts_with("module_scoped_type_classes__")
}

fn run_subdir(workspace_root: &Path, subdir: &str) {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    for case in snapshot_dir(workspace_root, subdir)
        .into_iter()
        .filter(|c| !is_excluded(&c.snapshot_name))
    {
        insta::with_settings!({
            snapshot_path => "../snapshots/examples_fixtures",
            prepend_module_to_snapshot => false,
            omit_expression => true,
        }, {
            insta::assert_snapshot!(case.snapshot_name, case.transcript);
        });
    }
}

#[test]
fn examples_module_graph() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "ModuleGraph");
}

#[test]
fn examples_modules() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "Modules");
}

#[test]
fn examples_aether() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "aether");
}

#[test]
fn examples_aoc() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "aoc");
}

#[test]
fn examples_async() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "async");
}

#[test]
fn examples_benchmarks() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "benchmarks");
}

#[test]
fn examples_compiler_errors() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "compiler_errors");
}

#[test]
fn examples_diagnostics() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "diagnostics");
}

#[test]
fn examples_effects() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "effects");
}

#[test]
fn examples_flow() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "flow");
}

#[test]
fn examples_functions() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "functions");
}

#[test]
fn examples_guide() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "guide");
}

#[test]
fn examples_guide_async() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "guide_async");
}

#[test]
fn examples_guide_type_system() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "guide_type_system");
}

#[test]
fn examples_http() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "http");
}

#[test]
fn examples_imports() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "imports");
}

#[test]
fn examples_io() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "io");
}

#[test]
fn examples_namespaces() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "namespaces");
}

#[test]
fn examples_optimizations() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "optimizations");
}

#[test]
fn examples_parser_errors() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "parser_errors");
}

#[test]
fn examples_patterns() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "patterns");
}

#[test]
fn examples_performance() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "performance");
}

#[test]
fn examples_primop() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "primop");
}

#[test]
fn examples_roots() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "roots");
}

#[test]
fn examples_runtime_boundaries() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "runtime_boundaries");
}

#[test]
fn examples_runtime_errors() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "runtime_errors");
}

#[test]
fn examples_sealing() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "sealing");
}

#[test]
fn examples_strict_types() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "strict_types");
}

#[test]
fn examples_tail_call() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "tail_call");
}

#[test]
fn examples_tests() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "tests");
}

#[test]
fn examples_type_classes() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "type_classes");
}

#[test]
fn examples_type_inference() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "type_inference");
}

#[test]
fn examples_type_system() {
    run_subdir(Path::new(env!("CARGO_MANIFEST_DIR")), "type_system");
}
