mod diagnostics_env;
#[path = "common/examples_snapshot.rs"]
mod examples_snapshot;

use std::path::Path;

#[test]
fn examples_basics_compile_snapshots() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cases = examples_snapshot::run_fixture_dir_snapshots(workspace_root, "examples/basics")
        .unwrap_or_else(|e| panic!("{e}"));

    for case in cases {
        insta::with_settings!({
            snapshot_path => "snapshots/examples_basics",
            prepend_module_to_snapshot => false,
            omit_expression => true,
        }, {
            insta::assert_snapshot!(case.snapshot_name, case.transcript);
        });
    }
}
