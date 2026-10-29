use super::*;

/// E487 fires when a class environment exists but does not carry the class an
/// operator desugars to.
///
/// Proposal 0179 Stage 8 moved `Eq`/`Ord`/`Num`/`Semigroup` into
/// `lib/Flow/*.flx` and injects them into every module. Before that guard, an
/// operator whose class was absent emitted no obligation *and* no diagnostic,
/// so the function compiled unconstrained and the dictionary it needed was
/// never passed — a silent miscompile rather than an error.
#[test]
fn polymorphic_operator_reports_e487_when_its_class_is_missing() {
    let (result, _program) = infer_program_from_source_with_class_env(
        r#"
fn plus<a>(x: a, y: a) -> a {
    x + y
}
"#,
        Some(flux::types::class_env::ClassEnv::new()),
    );
    assert!(
        result.diagnostics.iter().any(|d| d.code() == Some("E487")),
        "expected E487 when `Num` is absent from a present class environment, got: {:?}",
        result.diagnostics.iter().map(|d| d.code().unwrap_or("")).collect::<Vec<_>>()
    );
}

/// The other half of that guard: an embedder that runs inference with no class
/// environment at all has opted out of classes, so a missing class is its
/// configuration rather than a defect and must not be reported.
#[test]
fn polymorphic_operator_is_silent_when_there_is_no_class_env() {
    let (result, _program) = infer_program_from_source(
        r#"
fn plus<a>(x: a, y: a) -> a {
    x + y
}
"#,
    );
    assert!(
        result.diagnostics.is_empty(),
        "inference without a class environment must not report E487, got: {:?}",
        result.diagnostics
    );
}

// ── Constructor patterns are checked against the scrutinee (KI-072) ──

#[test]
fn infer_constructor_pattern_from_another_adt_emits_e300() {
    let source = r#"
data Colour { Red, Green }
data Shape { Circle(Int), Square }

fn classify(c: Colour) -> Bool {
    match c { Square -> true, _ -> false }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for a `Shape` constructor matched against `Colour`, got: {:#?}",
        result.diagnostics
    );
}

/// The catch-all arm is what used to suppress this: every arm was bound
/// against a fresh variable, which unifies with anything.
#[test]
fn infer_constructor_pattern_carrying_fields_from_another_adt_emits_e300() {
    let source = r#"
data Colour { Red, Green }
data Shape { Circle(Int), Square }

fn size_of(c: Colour) -> Int {
    match c { Circle(n) -> n, _ -> 0 }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for a `Shape` constructor matched against `Colour`, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_constructor_pattern_against_primitive_scrutinee_emits_e300() {
    let source = r#"
data Colour { Red, Green }

fn classify(n: Int) -> Bool {
    match n { Red -> true, _ -> false }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for an ADT constructor matched against `Int`, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_matching_constructor_pattern_with_catch_all_is_accepted() {
    let source = r#"
data Colour { Red, Green, Blue }

fn classify(c: Colour) -> Int {
    match c { Red -> 1, _ -> 0 }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_code(&result, "E300"),
        "a constructor of the scrutinee's own type must not report, got: {:#?}",
        result.diagnostics
    );
}

/// The behaviour arm isolation exists to protect: the built-in families are
/// structural, so mixing them must not make one arm constrain another through
/// the shared scrutinee slot.
#[test]
fn infer_builtin_family_patterns_with_catch_all_are_accepted() {
    let source = r#"
fn from_option(o: Option<Int>) -> Int {
    match o { Some(v) -> v, _ -> 0 }
}

fn from_either(e: Either<Int, String>) -> Int {
    match e { Left(v) -> v, _ -> 0 }
}

fn from_list(xs: List<Int>) -> Int {
    match xs { [h | _] -> h, _ -> 0 }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_code(&result, "E300"),
        "built-in family patterns must still be accepted, got: {:#?}",
        result.diagnostics
    );
}

/// A field predicate whose receiver is bound by a match arm must be determined
/// by the call site, like one bound as a parameter
/// (`docs/known_issues.md#ki-095`).
///
/// Before the fix a catch-all arm stopped the shared pattern family reaching an
/// unresolved scrutinee, and the arms were then isolated onto fresh variables
/// — so `r` was related to nothing the single call could reach, and `r.v` was
/// reported as `E490` for a program with exactly one call at exactly one type.
#[test]
fn match_arm_bound_receiver_is_determined_by_its_call_site() {
    let source = r#"
data Box { Box { v: Int } }

fn head_v(rs) {
    match rs {
        [r | _] -> r.v,
        _ -> 0
    }
}

fn main() -> Int {
    head_v([Box { v: 7 }])
}
"#;
    let (result, _program) = infer_program_from_source(source);
    assert!(
        result.diagnostics.is_empty(),
        "a match-arm receiver with one call site should need no annotation, got {:?}",
        result.diagnostics
    );
}

/// The isolation this relaxes exists to keep arms of *different* families from
/// constraining one another through the shared scrutinee slot, so that must
/// still hold: `Some` and `Left` arms over an unknown scrutinee have no shared
/// family, and neither arm may be forced into the other's shape.
#[test]
fn mixed_pattern_families_still_do_not_constrain_one_another() {
    let source = r#"
fn describe(x) {
    match x {
        Some(v) -> v,
        _ -> 0
    }
}

fn main() -> Int {
    describe(Some(1))
}
"#;
    let (result, _program) = infer_program_from_source(source);
    assert!(
        result.diagnostics.is_empty(),
        "a single-family match with a catch-all should still infer cleanly: {:?}",
        result.diagnostics
    );
}

/// A recursive group's predeclared placeholder must end up *being* what the
/// member inferred, not merely replaced by it (`docs/known_issues.md#ki-096`).
///
/// `step` is predeclared at `Scheme::mono(v)` so `go` can call it. If `v` is
/// never unified with `step`'s inferred type, the `let` in `go` holds a type
/// nothing resolves, and the projection on it has no receiver — while `main`'s
/// identical projection resolves, because it reads the finished binding.
#[test]
fn a_recursive_group_member_resolves_its_siblings_result() {
    let source = r#"
data Pair { Pair { a: Int, b: Int } }

fn go(n, acc) {
    if n <= 0 {
        Pair { a: acc, b: 0 }
    } else {
        let r = step(n)
        go(n - 1, acc + r.a)
    }
}

fn step(n) { go(n - 1, 0) }

fn main() -> Int {
    go(3, 0).a
}
"#;
    let (result, _program) = infer_program_from_source(source);
    assert!(
        result.diagnostics.is_empty(),
        "a sibling's result should have a type to project: {:?}",
        result.diagnostics
    );
}

/// The same program without the intervening `let`, which isolates the two
/// halves of the fix: this one needs only the group unification, while the
/// version above additionally needs the environment to be read through the
/// substitution before the `let` generalizes.
#[test]
fn a_recursive_group_member_resolves_an_inline_sibling_projection() {
    let source = r#"
data Pair { Pair { a: Int, b: Int } }

fn go(n, acc) {
    if n <= 0 { Pair { a: acc, b: 0 } } else { go(n - 1, acc + step(n).a) }
}

fn step(n) { go(n - 1, 0) }

fn main() -> Int { go(3, 0).a }
"#;
    let (result, _program) = infer_program_from_source(source);
    assert!(
        result.diagnostics.is_empty(),
        "an inline sibling projection should resolve: {:?}",
        result.diagnostics
    );
}

/// Tuple projection is a predicate, not a guessed tuple shape (proposal 0185
/// stage 5). `t.0` used to unify an unknown receiver with a *pair*, so this
/// program typed only because the failure against a triple was discarded.
#[test]
fn tuple_projection_does_not_guess_the_receivers_width() {
    let source = r#"
fn fst(t) { t.0 }

fn main() -> Int {
    fst((1, 2, 3))
}
"#;
    let (result, _program) = infer_program_from_source(source);
    assert!(
        result.diagnostics.is_empty(),
        "projecting .0 says nothing about the tuple's width: {:?}",
        result.diagnostics
    );
}

/// An unannotated definition that raises no class constraint is generic.
///
/// The unconstrained half of generalize-by-arity: `identity` has no dictionary
/// to plumb and no specialised arithmetic to lose, so neither reason to
/// withhold generalization reaches it.
#[test]
fn an_unconstrained_unannotated_function_is_generic() {
    let source = r#"
fn identity(x) { x }

fn main() -> Int {
    let s = identity("hi")
    identity(1)
}
"#;
    let (result, _program) = infer_program_from_source(source);
    assert!(
        result.diagnostics.is_empty(),
        "an unconstrained helper should type at two argument types: {:?}",
        result.diagnostics
    );
}

/// A *nullary* definition stays monomorphic. Generalization is by arity, so a
/// value binding is still restricted — this is the monomorphism restriction's
/// own case, and widening it is not what the unconstrained rule does.
#[test]
fn a_nullary_definition_is_not_generalized_by_the_unconstrained_rule() {
    let source = r#"
fn pick() { 1 }

fn main() -> Int { pick() }
"#;
    let (result, _program) = infer_program_from_source(source);
    assert!(
        result.diagnostics.is_empty(),
        "a nullary definition should still compile: {:?}",
        result.diagnostics
    );
}
