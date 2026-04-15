#[path = "support/semantic_matrix.rs"]
mod semantic_matrix;

use semantic_matrix::{combined_output, run_fixture};

fn output_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with('['))
        .filter(|line| !line.starts_with('•'))
        .filter(|line| !line.contains("Compiling"))
        .filter(|line| !line.contains("Running via"))
        .map(str::to_string)
        .collect()
}

#[test]
fn semantic_matrix_runtime_vm_cases_match_expected_output() {
    let cases: &[(&str, &[&str])] = &[
        ("primitives/integer_arithmetic.flx", &["42", "0", "11"]),
        ("functions/polymorphism.flx", &["7", "\"x\""]),
        (
            "functions/closures_and_recursion.flx",
            &["15", "120", "true"],
        ),
        ("collections/shapes.flx", &["3", "3", "9", "4"]),
        ("adts/generic_adts.flx", &["7", "9"]),
        ("modules/public_api/main.flx", &["42"]),
        ("modules/alias_access/main.flx", &["42"]),
        ("effects/basic_handle.flx", &["6"]),
        ("effects/hof_row_propagation.flx", &["9"]),
        ("type_classes/builtin_constraints.flx", &["true", "42"]),
        ("type_classes/custom_class.flx", &["true", "true"]),
        ("runtime_boundaries/typed_boundary.flx", &["42"]),
    ];

    for (fixture, expected_lines) in cases {
        let output = run_fixture(fixture, false);
        let text = combined_output(&output);
        assert!(
            output.status.success(),
            "VM run failed for {fixture}:\n{text}"
        );
        assert_eq!(
            output_lines(&text),
            *expected_lines,
            "VM output for {fixture}"
        );
    }
}

#[cfg(feature = "llvm")]
#[test]
fn semantic_matrix_runtime_native_matches_vm_output() {
    let probe = run_fixture("runtime_boundaries/typed_boundary.flx", true);
    let probe_text = combined_output(&probe);
    if probe_text.contains("native backend features require `llvm`") {
        eprintln!("skipping native parity check: {probe_text}");
        return;
    }

    let cases = [
        "primitives/integer_arithmetic.flx",
        "functions/polymorphism.flx",
        "functions/closures_and_recursion.flx",
        "collections/shapes.flx",
        "adts/generic_adts.flx",
        "modules/public_api/main.flx",
        "modules/alias_access/main.flx",
        "effects/basic_handle.flx",
        "effects/hof_row_propagation.flx",
        "type_classes/builtin_constraints.flx",
        "type_classes/custom_class.flx",
        "runtime_boundaries/typed_boundary.flx",
    ];

    for fixture in cases {
        let vm = run_fixture(fixture, false);
        let vm_text = combined_output(&vm);
        assert!(
            vm.status.success(),
            "VM run failed for {fixture}:\n{vm_text}"
        );

        let native = run_fixture(fixture, true);
        let native_text = combined_output(&native);
        assert!(
            native.status.success(),
            "native run failed for {fixture}:\n{native_text}"
        );

        assert_eq!(
            output_lines(&native_text),
            output_lines(&vm_text),
            "native output should match VM for {fixture}"
        );
    }
}
