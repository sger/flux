mod diagnostics_env;
#[path = "support/examples_snapshot.rs"]
mod examples_snapshot;

use std::path::Path;

#[test]
fn parser_error_fixtures_snapshot() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cases =
        examples_snapshot::run_fixture_dir_snapshots(workspace_root, "examples/parser_errors")
            .unwrap_or_else(|e| panic!("{e}"));

    for case in cases.into_iter().filter(|case| {
        !case.snapshot_name.contains("max_errors")
            && !case.snapshot_name.contains("kitchen_sink_many_errors")
            && !case.snapshot_name.contains("long_functions_many_errors")
            && !case
                .snapshot_name
                .contains("long_functions_clean_many_errors")
    }) {
        insta::with_settings!({
            snapshot_path => "snapshots/parser_error_fixtures",
            prepend_module_to_snapshot => false,
            omit_expression => true,
        }, {
            insta::assert_snapshot!(case.snapshot_name, case.transcript);
        });
    }
}
