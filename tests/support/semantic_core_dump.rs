use std::path::{Path, PathBuf};

use flux::{
    bytecode::compiler::Compiler,
    core::display::CoreDisplayMode,
    diagnostics::{Diagnostic, render_diagnostics},
    syntax::{interner::Interner, lexer::Lexer, parser::Parser},
};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn semantic_fixture_root() -> PathBuf {
    workspace_root().join("tests/fixtures/semantic_types")
}

fn semantic_fixture_path(rel: &str) -> PathBuf {
    semantic_fixture_root().join(rel)
}

struct CompiledFixture {
    compiler: Compiler,
    program: flux::syntax::program::Program,
    source: String,
}

fn parse_source(source: &str) -> (flux::syntax::program::Program, Interner) {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {}",
        render_diagnostics(&parser.errors, Some(source), None)
    );
    (program, parser.take_interner())
}

fn compile_single_file_fixture(rel: &str) -> Result<CompiledFixture, Vec<Diagnostic>> {
    let source = std::fs::read_to_string(semantic_fixture_path(rel))
        .unwrap_or_else(|err| panic!("failed to read fixture {rel}: {err}"));
    let (program, interner) = parse_source(&source);
    let mut compiler = Compiler::new_with_interner(rel, interner.clone());
    match compiler.compile(&program) {
        Ok(()) => Ok(CompiledFixture {
            compiler,
            program,
            source,
        }),
        Err(diags) => Err(diags),
    }
}

pub fn dump_core_debug_fixture(rel: &str) -> String {
    let compiled = compile_single_file_fixture(rel)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(rel), None)));
    let mut compiler = compiled.compiler;
    compiler
        .dump_core_with_opts(&compiled.program, false, CoreDisplayMode::Debug)
        .unwrap_or_else(|diag| {
            panic!(
                "{}",
                render_diagnostics(&[diag], Some(&compiled.source), None)
            )
        })
}
