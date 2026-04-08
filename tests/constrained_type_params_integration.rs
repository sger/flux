use flux::{
    bytecode::compiler::Compiler,
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
    compiler.set_strict_types(true);

    let diags = compiler
        .compile_with_opts(&program, false, false)
        .expect_err("missing explicit-bound instance should fail compilation");
    assert!(
        diags.iter().any(|d| d.code().as_deref() == Some("E444")),
        "expected missing explicit-bound instance to report E444, got: {diags:?}"
    );
}
