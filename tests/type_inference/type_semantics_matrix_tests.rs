#[path = "../support/semantic_infer.rs"]
mod semantic_infer;

use flux::diagnostics::render_diagnostics;
use semantic_infer::{
    assert_module_member_schemes, assert_named_schemes, compile_module_fixture,
    compile_single_file_fixture, first_error_code,
};

#[test]
fn semantic_matrix_infers_expected_named_schemes_across_categories() {
    let cases: &[(&str, &[(&str, &str)])] = &[
        (
            "primitives/integer_arithmetic.flx",
            &[
                ("add1", "(Int) -> Int"),
                ("choose", "(Bool) -> Int"),
                ("do_result", "() -> Int"),
            ],
        ),
        (
            "primitives/float_string_pipeline.flx",
            &[
                ("half", "(Float) -> Float"),
                ("greet", "(String) -> String"),
                ("piped", "(Int) -> Int"),
            ],
        ),
        (
            "functions/polymorphism.flx",
            &[
                ("id", "forall a, b. (a) -> a with |b"),
                ("const_", "forall a, b, c. (a, b) -> a with |c"),
                (
                    "apply",
                    "forall a, b, c. ((a) -> b with |c, a) -> b with |c",
                ),
            ],
        ),
        (
            "functions/closures_and_recursion.flx",
            &[
                ("make_adder", "forall a, b. (a) -> (a) -> a with |b"),
                ("fact", "(Int) -> Int"),
                ("evenish", "(Int) -> Bool"),
                ("oddish", "(Int) -> Bool"),
            ],
        ),
        (
            "collections/shapes.flx",
            &[
                ("list_nums", "() -> List<Int>"),
                ("array_nums", "() -> Array<Int>"),
                ("options", "forall a. (Bool, a) -> Option<a>"),
                ("either_of", "forall a, b. (Bool, a, b) -> Either<a, b>"),
                ("scores", "() -> Map<String, Int>"),
            ],
        ),
        (
            "adts/generic_adts.flx",
            &[
                ("wrap", "forall a. (a) -> Box<a>"),
                ("unwrap_or_zero", "forall a. (Result<Int, a>) -> Int"),
                ("nested", "forall a. (Result<Box<Int>, a>) -> Int"),
            ],
        ),
        (
            "patterns/option_tuple_patterns.flx",
            &[
                ("option_to_int", "(Option<Int>) -> Int"),
                ("tuple_sum", "forall a. ((a, a)) -> a"),
            ],
        ),
        (
            "runtime_boundaries/typed_boundary.flx",
            &[("inc", "(Int) -> Int")],
        ),
    ];

    for (fixture, expected) in cases {
        assert_named_schemes(fixture, expected);
    }
}

#[test]
fn semantic_matrix_tracks_exported_module_member_schemes() {
    assert_module_member_schemes(
        "modules/public_api/main.flx",
        &[
            ("Math", "id", "forall a. (a) -> a"),
            ("Math", "inc", "(Int) -> Int"),
        ],
    );
    assert_module_member_schemes(
        "modules/alias_access/main.flx",
        &[("Math", "twice", "(Int) -> Int")],
    );
}

#[test]
fn semantic_matrix_negative_cases_report_expected_primary_code() {
    let single_file_cases = [
        ("collections/heterogeneous_array_error.flx", "E300"),
        ("adts/constructor_arity_error.flx", "E082"),
        ("patterns/non_exhaustive_bool_error.flx", "E015"),
        ("effects/incomplete_handler_error.flx", "E402"),
        ("type_classes/missing_instance_error.flx", "E444"),
        ("runtime_boundaries/annotation_mismatch_error.flx", "E300"),
    ];

    for (fixture, expected_code) in single_file_cases {
        let diags = match compile_single_file_fixture(fixture) {
            Ok(_) => panic!("fixture should fail to compile: {fixture}"),
            Err(diags) => diags,
        };
        assert_eq!(
            first_error_code(&diags),
            expected_code,
            "{fixture} diagnostics:\n{}",
            render_diagnostics(&diags, None, None)
        );
    }

    let module_diags = match compile_module_fixture("modules/private_member/main.flx") {
        Ok(_) => panic!("private member fixture should fail"),
        Err(diags) => diags,
    };
    assert_eq!(
        first_error_code(&module_diags),
        "E011",
        "modules/private_member/main.flx diagnostics:\n{}",
        render_diagnostics(&module_diags, None, None)
    );
}
