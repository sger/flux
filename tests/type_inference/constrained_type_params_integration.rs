use flux::{
    compiler::Compiler,
    core::display::CoreDisplayMode,
    syntax::{lexer::Lexer, parser::Parser},
};

fn compiler_for(src: &str) -> (flux::syntax::program::Program, Compiler) {
    let mut parser = Parser::new(Lexer::new(src));
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let compiler = Compiler::new_with_interner("<test>", interner);
    (program, compiler)
}

#[test]
fn constrained_type_params_dump_core_emits_dictionary_elaboration() {
    let (program, mut compiler) = compiler_for(
        r#"
class Eq<a> {
    fn eq(x: a, y: a) -> Bool
}

instance Eq<Int> {
    fn eq(x, y) { true }
}

fn same<A: Eq>(x: A, y: A) -> Bool { eq(x, y) }

fn main() {
    same(1, 2)
}
"#,
    );

    compiler
        .compile_with_opts(&program, false, false)
        .expect("compilation should succeed");
    let dumped = compiler
        .dump_core_with_opts(&program, false, CoreDisplayMode::Readable)
        .expect("core dump should succeed");

    assert!(
        dumped.contains("__dict_Eq_Int"),
        "expected explicit bounds to elaborate concrete dictionaries, got:\n{dumped}"
    );
    assert!(
        dumped.contains("__dict_Eq"),
        "expected constrained function to receive a dictionary parameter, got:\n{dumped}"
    );
}

#[test]
fn constrained_multi_bounds_compile_through_full_pipeline() {
    let (program, mut compiler) = compiler_for(
        r#"
fn describe<A: Eq + Show>(x: A, y: A) -> String {
    "ok"
}

fn main() {
    describe(1, 2)
}
"#,
    );

    compiler
        .compile_with_opts(&program, false, false)
        .expect("multi-bound constrained generics should compile");
}

#[test]
fn constrained_type_params_missing_instance_fails_compile() {
    let (program, mut compiler) = compiler_for(
        r#"
data Color { Red, Blue }

class Eq<a> {
    fn eq(x: a, y: a) -> Bool
}

instance Eq<Int> {
    fn eq(x, y) { true }
}

fn same<A: Eq>(x: A, y: A) -> Bool { eq(x, y) }

fn main() {
    same(Red, Blue)
}
"#,
    );
    let diags = compiler
        .compile_with_opts(&program, false, false)
        .expect_err("missing explicit-bound instance should fail compilation");
    assert!(
        diags.iter().any(|d| d.code() == Some("E444")),
        "expected missing explicit-bound instance to report E444, got: {diags:?}"
    );
}

#[test]
fn generic_ord_operator_compiles_without_strict_types() {
    let (program, mut compiler) = compiler_for(
        r#"
fn max_of<A: Ord>(x: A, y: A) -> A {
    if x > y { x } else { y }
}

fn main() {
    max_of(3, 7)
}
"#,
    );

    compiler
        .compile_with_opts(&program, false, false)
        .expect("generic Ord operator should compile without strict-types");
}

#[test]
fn generic_eq_and_num_operators_compile_without_strict_types() {
    let (program, mut compiler) = compiler_for(
        r#"
fn different<A: Eq>(x: A, y: A) -> Bool {
    x != y
}

fn half<A: Num>(x: A, y: A) -> A {
    x / y
}

fn main() {
    if different(10, 20) { half(8, 2) } else { 0 }
}
"#,
    );

    compiler
        .compile_with_opts(&program, false, false)
        .expect("generic Eq/Num operators should compile without strict-types");
}

#[test]
fn explicit_num_bound_survives_defaulting_and_elaborates_dictionary() {
    let (program, mut compiler) = compiler_for(
        r#"
fn half<A: Num>(x: A, y: A) -> A {
    x / y
}

fn main() {
    half(8, 2)
}
"#,
    );

    compiler
        .compile_with_opts(&program, false, false)
        .expect("explicit Num bound should remain constrained");
    let dumped = compiler
        .dump_core_with_opts(&program, false, CoreDisplayMode::Readable)
        .expect("core dump should succeed");

    assert!(
        dumped.contains("__dict_Num"),
        "expected explicit Num bound to elaborate a dictionary parameter, got:\n{dumped}"
    );
}

#[test]
fn constrained_operator_missing_instance_fails_without_strict_types() {
    let (program, mut compiler) = compiler_for(
        r#"
data Color { Red, Blue }

fn same<A: Eq>(x: A, y: A) -> Bool {
    x == y
}

fn main() {
    same(Red, Blue)
}
"#,
    );

    let diags = compiler
        .compile_with_opts(&program, false, false)
        .expect_err("missing instance should fail even without strict-types");
    assert!(
        diags.iter().any(|d| d.code() == Some("E444")),
        "expected non-strict missing instance to report E444, got: {diags:?}"
    );
}

/// Proposal 0168: polymorphic dispatch over a higher-kinded (constructor-headed)
/// type class constraint must route through dictionary elaboration instead of
/// degrading into the polymorphic-stub panic.
///
/// A function `fn poly<F: Functor, A>(container: F<A>, f: (A) -> A) -> F<A>`
/// must, when invoked with a concrete `List<Int>` container, thread the
/// `__dict_Functor_List` dictionary through to the callee.
#[test]
fn hkt_constrained_polymorphic_call_elaborates_dictionary() {
    let (program, mut compiler) = compiler_for(
        r#"
class Functor<f> {
    fn fmap<a, b>(x: f<a>, func: (a) -> b) -> f<b>
}

instance Functor<List> {
    fn fmap(xs, func) {
        fn go(ys) {
            match ys {
                [] -> [],
                [h | t] -> [func(h) | go(t)]
            }
        }
        go(xs)
    }
}

fn double_val(x: Int) -> Int { x * 2 }

fn double_all<f: Functor, a>(container: f<a>, d: (a) -> a) -> f<a> {
    fmap(container, d)
}

fn main() {
    let xs = [1, 2, 3]
    double_all(xs, double_val)
}
"#,
    );

    compiler
        .compile_with_opts(&program, false, false)
        .expect("polymorphic HKT dispatch should compile");
    let dumped = compiler
        .dump_core_with_opts(&program, false, CoreDisplayMode::Readable)
        .expect("core dump should succeed");

    assert!(
        dumped.contains("__dict_Functor_List"),
        "expected concrete Functor<List> dictionary at the HKT call site, got:\n{dumped}"
    );
    assert!(
        dumped.contains("double_all(__dict_Functor_List"),
        "expected `double_all` call to receive the concrete dictionary as its \
         first argument, got:\n{dumped}"
    );
}
