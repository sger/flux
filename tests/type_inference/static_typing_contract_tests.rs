use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use flux::cfg::validate_ir;
use flux::compiler::Compiler;
use flux::core::to_ir::lower_core_to_ir;
use flux::core::{display::CoreDisplayMode, lower_ast::lower_program_ast};
use flux::diagnostics::render_diagnostics;
use flux::syntax::{
    expression::ExprId, interner::Interner, lexer::Lexer, module_graph::ModuleGraph,
    parser::Parser,
};
use flux::types::infer_type::InferType;

static MODULE_AUDIT_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn parse(input: &str) -> (flux::syntax::program::Program, Interner) {
    let mut parser = Parser::new(Lexer::new(input));
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    (program, interner)
}

fn compile_err_code(input: &str) -> String {
    let (program, interner) = parse(input);
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    let err = compiler
        .compile(&program)
        .expect_err("expected compile error");
    err.first()
        .map(|d| d.code().unwrap_or("").to_string())
        .unwrap_or_default()
}

fn compile_ok_static_typing(input: &str) {
    let (program, interner) = parse(input);
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
}

fn dump_core_debug(input: &str) -> String {
    let (program, interner) = parse(input);
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    compiler
        .dump_core_with_opts(&program, false, CoreDisplayMode::Debug)
        .expect("dump_core_with_opts should succeed")
}

fn parse_and_infer(
    input: &str,
) -> (
    flux::syntax::program::Program,
    HashMap<ExprId, InferType>,
    Interner,
) {
    let (program, interner) = parse(input);
    let mut compiler = Compiler::new_with_interner("<unknown>", interner.clone());
    let hm_expr_types = compiler.infer_expr_types_for_program(&program);
    (program, hm_expr_types, interner)
}

fn compile_module_fixture_strict(rel_path: &str) {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = workspace_root.join(rel_path);
    let module_name = rel_path
        .strip_prefix("lib/")
        .expect("audit fixtures should live under lib/")
        .strip_suffix(".flx")
        .expect("audit fixtures should be .flx files")
        .replace(['\\', '/'], ".");

    let audit_id = MODULE_AUDIT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_dir = workspace_root.join(format!("target/tmp/base_effect_audit/{audit_id}"));
    std::fs::create_dir_all(&temp_dir)
        .unwrap_or_else(|err| panic!("failed to create {}: {err}", temp_dir.display()));
    let entry_path = temp_dir.join("main.flx");
    let entry_source = format!("import {module_name} exposing (..)\n\nfn main() {{ () }}\n");
    std::fs::write(&entry_path, &entry_source)
        .unwrap_or_else(|err| panic!("failed to write {}: {err}", entry_path.display()));

    let mut parser = Parser::new(Lexer::new(&entry_source));
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "{}",
        render_diagnostics(&parser.errors, Some(&entry_source), None)
    );

    let roots = vec![temp_dir.clone(), workspace_root.join("lib")];
    let graph_result =
        ModuleGraph::build_with_entry_and_roots(&entry_path, &program, parser.take_interner(), &roots);
    assert!(
        graph_result.diagnostics.is_empty(),
        "module graph diagnostics for {} via {}:\n{}",
        fixture.display(),
        entry_path.display(),
        render_diagnostics(&graph_result.diagnostics, Some(&entry_source), None)
    );

    let mut compiler =
        Compiler::new_with_interner(entry_path.to_string_lossy().to_string(), graph_result.interner);
    let entry_module_path = graph_result
        .graph
        .entry_node()
        .expect("audit graph should have an entry node")
        .path
        .clone();
    let audited_module_path = std::fs::canonicalize(&fixture).unwrap_or(fixture.clone());
    for node in graph_result.graph.topo_order() {
        compiler.set_file_path(node.path.to_string_lossy().to_string());
        compiler.set_current_module_kind(node.kind);
        let strict_for_node = node.path == entry_module_path || node.path == audited_module_path;
        compiler.set_strict_mode(strict_for_node);
        compiler.set_strict_require_main(node.path == entry_module_path);
        if let Err(diags) = compiler.compile(&node.program) {
            let node_source = std::fs::read_to_string(&node.path)
                .unwrap_or_else(|_| entry_source.clone());
            panic!(
                "strict compile failed for {}:\n{}",
                node.path.display(),
                render_diagnostics(&diags, Some(&node_source), None)
            );
        }
    }
}

#[test]
fn dynamic_top_type_escape_hatch_is_rejected_in_source_annotations() {
    let dynamic_top = ["A", "n", "y"].concat();
    let src = format!("fn id(x: {dynamic_top}) -> Int {{ 0 }}");
    let code = compile_err_code(&src);
    assert_eq!(code, "E423");
}

#[test]
fn static_typing_accepts_polymorphic_identity_without_dynamic_fallback() {
    compile_ok_static_typing("fn id(x) { x }");
}

#[test]
fn debug_core_dump_shows_explicit_type_residue_without_dynamic() {
    let core = dump_core_debug(
        r#"
fn id<T>(x: T) -> T { x }

fn main() {
    let n = id(1)
    let s = id("hi")
    n
}
"#,
    );

    assert!(
        core.contains("letrec id : "),
        "expected debug Core dump to preserve explicit type-variable residue, got:\n{core}"
    );
    assert!(
        core.contains("letrec id : forall a. (a) -> a ="),
        "expected canonical debug Core dump to render explicit quantified names, got:\n{core}"
    );
    assert!(
        !core.contains("Dynamic"),
        "debug Core dump should not regress to semantic Dynamic placeholders, got:\n{core}"
    );
}

#[test]
fn core_to_cfg_lowering_preserves_static_rep_contract() {
    let (program, hm_expr_types, _interner) = parse_and_infer(
        r#"
fn choose(x: Int) -> Int {
    if x > 0 { 1 } else { 0 }
}

fn main() {
    choose(5)
}
"#,
    );
    let core = lower_program_ast(&program, &hm_expr_types);
    let ir = lower_core_to_ir(&core);
    validate_ir(&ir).expect("maintained Core->CFG path should preserve rep/semantic contract");
}

#[test]
fn base_effect_audit_strict_effect_modules() {
    for rel_path in [
        "lib/Flow/Effects.flx",
        "lib/Flow/IO.flx",
        "lib/Flow/Assert.flx",
        "lib/Flow/FTest.flx",
    ] {
        compile_module_fixture_strict(rel_path);
    }
}
