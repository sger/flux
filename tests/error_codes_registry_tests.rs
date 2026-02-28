use flux::diagnostics::position::{Position, Span};
use flux::diagnostics::{
    ERROR_CODES, LabelStyle, call_arg_type_mismatch, fun_arity_mismatch, fun_param_type_mismatch,
    fun_return_annotation_mismatch, fun_return_type_mismatch, if_branch_type_mismatch,
    let_annotation_type_mismatch, lookup_error_code, match_arm_type_mismatch, wrong_argument_count,
};

fn span(line: usize, start_col: usize, end_col: usize) -> Span {
    Span::new(Position::new(line, start_col), Position::new(line, end_col))
}

#[test]
fn registry_has_unique_codes() {
    let mut codes = std::collections::HashSet::new();
    for item in ERROR_CODES {
        assert!(
            codes.insert(item.code),
            "duplicate error code in registry: {}",
            item.code
        );
    }
}

#[test]
fn registry_get_finds_codes() {
    for item in ERROR_CODES {
        let found = lookup_error_code(item.code).expect("code missing from registry");
        assert_eq!(found.title, item.title);
    }
}

#[test]
fn e056_title_is_repurposed_for_wrong_argument_count() {
    let e056 = lookup_error_code("E056").expect("E056 must exist");
    assert_eq!(e056.title, "WRONG NUMBER OF ARGUMENTS");
}

#[test]
fn wrong_argument_count_constructor_shape() {
    let diag = wrong_argument_count(
        "test.flx".to_string(),
        span(6, 11, 24),
        "add",
        2,
        3,
        Some(span(1, 4, 7)),
    );
    assert_eq!(diag.code(), Some("E056"));
    assert_eq!(diag.title(), "WRONG NUMBER OF ARGUMENTS");
    assert!(
        diag.labels()
            .iter()
            .any(|l| l.style == LabelStyle::Primary && !l.text.is_empty()),
        "expected a non-empty primary label"
    );
    assert!(
        diag.labels()
            .iter()
            .any(|l| l.style == LabelStyle::Secondary && !l.text.is_empty()),
        "expected a non-empty secondary label when definition span is provided"
    );
    assert!(
        diag.hints()
            .iter()
            .any(|h| h.text.contains("Remove 1 extra argument(s)")),
        "expected actionable help hint for too-many-args diagnostic"
    );
}

#[test]
fn e300_constructor_shapes_have_primary_labels() {
    let if_diag = if_branch_type_mismatch(
        "test.flx".to_string(),
        span(4, 9, 11),
        span(6, 9, 15),
        "Int",
        "String",
    );
    assert_eq!(if_diag.code(), Some("E300"));
    assert_eq!(if_diag.title(), "TYPE UNIFICATION ERROR");
    assert!(
        if_diag
            .labels()
            .iter()
            .any(|l| l.style == LabelStyle::Primary && !l.text.is_empty())
    );

    let match_diag = match_arm_type_mismatch(
        "test.flx".to_string(),
        span(3, 16, 21),
        span(4, 13, 22),
        "Int",
        "String",
        2,
    );
    assert_eq!(match_diag.code(), Some("E300"));
    assert!(
        match_diag
            .labels()
            .iter()
            .any(|l| l.style == LabelStyle::Primary && !l.text.is_empty())
    );

    let ret_diag =
        fun_return_type_mismatch("test.flx".to_string(), span(8, 20, 23), "Int", "String");
    assert_eq!(ret_diag.code(), Some("E300"));
    assert_eq!(
        ret_diag.message(),
        Some("Function return types do not match: expected `Int`, found `String`.")
    );
    assert!(
        ret_diag
            .labels()
            .iter()
            .any(|l| l.style == LabelStyle::Primary && !l.text.is_empty())
    );

    let param_diag =
        fun_param_type_mismatch("test.flx".to_string(), span(8, 15, 21), 1, "Int", "String");
    assert_eq!(param_diag.code(), Some("E300"));
    assert_eq!(
        param_diag.message(),
        Some("Function parameter 1 type does not match: expected `Int`, found `String`.")
    );
    assert!(
        param_diag
            .labels()
            .iter()
            .any(|l| l.style == LabelStyle::Primary && !l.text.is_empty())
    );

    let arity_diag = fun_arity_mismatch("test.flx".to_string(), span(8, 9, 26), 2, 1);
    assert_eq!(arity_diag.code(), Some("E300"));
    assert!(
        arity_diag
            .labels()
            .iter()
            .any(|l| l.style == LabelStyle::Primary && !l.text.is_empty())
    );

    let call_arg_diag = call_arg_type_mismatch(
        "test.flx".to_string(),
        span(6, 11, 13),
        Some("greet"),
        1,
        Some(span(1, 10, 16)),
        "String",
        "Int",
    );
    assert_eq!(call_arg_diag.code(), Some("E300"));
    assert!(
        call_arg_diag
            .message()
            .is_some_and(|m| m.contains("argument to `greet` has the wrong type"))
    );
    assert!(
        call_arg_diag
            .labels()
            .iter()
            .any(|l| l.style == LabelStyle::Secondary && !l.text.is_empty())
    );

    let let_diag = let_annotation_type_mismatch(
        "test.flx".to_string(),
        span(1, 8, 11),
        span(1, 14, 21),
        "x",
        "Int",
        "String",
    );
    assert_eq!(let_diag.code(), Some("E300"));
    assert!(
        let_diag
            .labels()
            .iter()
            .any(|l| l.style == LabelStyle::Primary && !l.text.is_empty())
    );
    assert!(
        let_diag
            .labels()
            .iter()
            .any(|l| l.style == LabelStyle::Secondary && !l.text.is_empty())
    );

    let ret_ann_diag = fun_return_annotation_mismatch(
        "test.flx".to_string(),
        span(1, 22, 25),
        span(3, 5, 11),
        "add",
        "Int",
        "String",
    );
    assert_eq!(ret_ann_diag.code(), Some("E300"));
    assert!(
        ret_ann_diag
            .message()
            .is_some_and(|m| m.contains("return value of `add`"))
    );
}
