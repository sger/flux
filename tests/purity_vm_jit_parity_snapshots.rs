mod diagnostics_env;
#[path = "support/purity_parity.rs"]
mod purity_parity;

use std::path::Path;

#[test]
fn purity_vm_jit_parity_snapshots() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let flux_bin = Path::new(env!("CARGO_BIN_EXE_flux"));

    for case in purity_parity::curated_cases() {
        let vm = purity_parity::run_case(workspace_root, flux_bin, &case, false)
            .unwrap_or_else(|e| panic!("vm run failed for {}: {e}", case.path));
        let jit = purity_parity::run_case(workspace_root, flux_bin, &case, true)
            .unwrap_or_else(|e| panic!("jit run failed for {}: {e}", case.path));

        if case.expect_compile_error {
            assert!(
                !vm.tuples.is_empty(),
                "expected vm compile diagnostics for {}, got none\n{}",
                case.path,
                vm.normalized_output
            );
            assert!(
                !jit.tuples.is_empty(),
                "expected jit compile diagnostics for {}, got none\n{}",
                case.path,
                jit.normalized_output
            );
        }

        // Parity freeze contract is tuple-level (`code`, `title`, `primary label`).
        // Full rendered text can differ by backend-specific formatting and is non-blocking.
        assert_eq!(
            vm.tuples,
            jit.tuples,
            "VM/JIT diagnostic tuple mismatch for {}\n{}",
            case.path,
            purity_parity::parity_transcript(&case, &vm, &jit)
        );

        let snapshot_name = purity_parity::snapshot_name(&case);
        let transcript = purity_parity::parity_transcript(&case, &vm, &jit);

        insta::with_settings!({
            snapshot_path => "snapshots/purity_parity",
            prepend_module_to_snapshot => false,
            omit_expression => true,
        }, {
            insta::assert_snapshot!(snapshot_name, transcript);
        });
    }
}
