use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use flux::{
    bytecode::compiler::Compiler,
    diagnostics::Diagnostic,
    syntax::{
        interner::Interner, lexer::Lexer, module_graph::ModuleGraph, parser::Parser,
        program::Program,
    },
};

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut root = std::env::temp_dir();
    root.push(format!("flux_module_graph_tests_{}_{}", label, nanos));
    fs::create_dir_all(&root).expect("create temp root");
    root
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write file");
}

fn parse_program(source: &str) -> (Program, Interner) {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    (program, interner)
}

fn first_code(diags: &[Diagnostic]) -> String {
    diags
        .first()
        .and_then(|d| d.code().map(|s| s.to_string()))
        .unwrap_or_default()
}

fn compile_with_graph(
    entry_path: &Path,
    entry_program: &Program,
    interner: Interner,
    roots: &[PathBuf],
) -> Result<(), Vec<Diagnostic>> {
    let result =
        ModuleGraph::build_with_entry_and_roots(entry_path, entry_program, interner, roots);
    if !result.diagnostics.is_empty() {
        return Err(result.diagnostics);
    }
    let mut compiler =
        Compiler::new_with_interner(entry_path.display().to_string(), result.interner);
    let mut errors = Vec::new();
    for node in result.graph.topo_order() {
        compiler.set_file_path(node.path.to_string_lossy().to_string());
        compiler.set_current_module_kind(node.kind);
        if let Err(mut diags) = compiler.compile(&node.program) {
            errors.append(&mut diags);
            continue;
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[test]
fn importing_script_is_error() {
    let root = temp_root("script_import");
    let script_path = root.join("Script.flx");
    write_file(&script_path, "let x = 1;");

    let entry_path = root.join("Main.flx");
    let entry_source = "import Script\n1;";
    write_file(&entry_path, entry_source);
    let (program, interner) = parse_program(entry_source);

    let result = ModuleGraph::build_with_entry_and_roots(&entry_path, &program, interner, &[root]);
    assert!(!result.diagnostics.is_empty(), "expected diagnostics");
    assert_eq!(first_code(&result.diagnostics), "E022");
}

#[test]
fn module_path_mismatch_is_error() {
    let root = temp_root("path_mismatch");
    let module_path = root.join("Data").join("List.flx");
    write_file(&module_path, "module Data.Other { fn value() { 1; } }");

    let entry_path = root.join("Main.flx");
    let entry_source = "import Data.List\n1;";
    write_file(&entry_path, entry_source);
    let (program, interner) = parse_program(entry_source);

    let result = ModuleGraph::build_with_entry_and_roots(&entry_path, &program, interner, &[root]);
    assert!(!result.diagnostics.is_empty(), "expected diagnostics");
    assert_eq!(first_code(&result.diagnostics), "E024");
}

#[test]
fn module_file_with_script_code_is_error() {
    let root = temp_root("module_script");
    let module_path = root.join("Mixed.flx");
    write_file(&module_path, "module Mixed { fn value() { 1; } }\n1;");

    let entry_path = root.join("Main.flx");
    let entry_source = "import Mixed\n1;";
    write_file(&entry_path, entry_source);
    let (program, interner) = parse_program(entry_source);

    let result = ModuleGraph::build_with_entry_and_roots(&entry_path, &program, interner, &[root]);
    assert!(!result.diagnostics.is_empty(), "expected diagnostics");
    assert_eq!(first_code(&result.diagnostics), "E028");
}

#[test]
fn alias_import_compiles() {
    let root = temp_root("alias_import");
    let module_path = root.join("Data").join("MyFile.flx");
    write_file(
        &module_path,
        "module Data.MyFile { public fn value() { 1; } }",
    );

    let entry_path = root.join("Main.flx");
    let entry_source = "import Data.MyFile as Alias\nAlias.value();";
    write_file(&entry_path, entry_source);
    let (program, interner) = parse_program(entry_source);

    compile_with_graph(&entry_path, &program, interner, &[root])
        .expect("expected alias import to compile");
}

#[test]
fn import_except_on_module_hides_excluded_member() {
    let root = temp_root("import_except_module");
    let module_path = root.join("Data").join("MyFile.flx");
    write_file(
        &module_path,
        "module Data.MyFile { public fn keep() { 1; } public fn drop() { 2; } }",
    );

    let entry_path = root.join("Main.flx");
    let entry_source = "import Data.MyFile except [drop]\nData.MyFile.keep();\nData.MyFile.drop();";
    write_file(&entry_path, entry_source);
    let (program, interner) = parse_program(entry_source);

    let result = compile_with_graph(&entry_path, &program, interner, &[root]);
    let diags = result.expect_err("expected excluded member access to fail");
    assert_eq!(first_code(&diags), "E012");
}

#[test]
fn import_except_on_module_keeps_other_members() {
    let root = temp_root("import_except_module_ok");
    let module_path = root.join("Data").join("MyFile.flx");
    write_file(
        &module_path,
        "module Data.MyFile { public fn keep() { 1; } public fn drop() { 2; } }",
    );

    let entry_path = root.join("Main.flx");
    let entry_source = "import Data.MyFile except [drop]\nData.MyFile.keep();";
    write_file(&entry_path, entry_source);
    let (program, interner) = parse_program(entry_source);

    compile_with_graph(&entry_path, &program, interner, &[root])
        .expect("expected non-excluded member access to compile");
}

#[test]
fn import_except_with_alias_hides_excluded_member() {
    let root = temp_root("import_except_alias");
    let module_path = root.join("Data").join("MyFile.flx");
    write_file(
        &module_path,
        "module Data.MyFile { public fn keep() { 1; } public fn drop() { 2; } }",
    );

    let entry_path = root.join("Main.flx");
    let entry_source = "import Data.MyFile as M except [drop]\nM.drop();";
    write_file(&entry_path, entry_source);
    let (program, interner) = parse_program(entry_source);

    let result = compile_with_graph(&entry_path, &program, interner, &[root]);
    let diags = result.expect_err("expected excluded alias member access to fail");
    assert_eq!(first_code(&diags), "E012");
}

#[test]
fn duplicate_module_across_roots_is_error() {
    let root_a = temp_root("dupe_root_a");
    let root_b = temp_root("dupe_root_b");
    let module_rel = Path::new("Dup").join("Mod.flx");
    write_file(
        &root_a.join(&module_rel),
        "module Dup.Mod { fn value() { 1; } }",
    );
    write_file(
        &root_b.join(&module_rel),
        "module Dup.Mod { fn value() { 2; } }",
    );

    let entry_path = root_a.join("Main.flx");
    let entry_source = "import Dup.Mod\nDup.Mod.value();";
    write_file(&entry_path, entry_source);
    let (program, interner) = parse_program(entry_source);

    let result =
        ModuleGraph::build_with_entry_and_roots(&entry_path, &program, interner, &[root_a, root_b]);
    assert!(!result.diagnostics.is_empty(), "expected diagnostics");
    assert_eq!(first_code(&result.diagnostics), "E027");
}

#[test]
fn import_cycle_is_error() {
    let root = temp_root("import_cycle");
    let module_a = root.join("A.flx");
    let module_b = root.join("B.flx");
    write_file(&module_a, "import B\nmodule A { fn value() { 1; } }");
    write_file(&module_b, "import A\nmodule B { fn value() { 2; } }");

    let entry_path = root.join("Main.flx");
    let entry_source = "import A\nA.value();";
    write_file(&entry_path, entry_source);
    let (program, interner) = parse_program(entry_source);

    let result = ModuleGraph::build_with_entry_and_roots(&entry_path, &program, interner, &[root]);
    assert!(!result.diagnostics.is_empty(), "expected diagnostics");
    assert_eq!(first_code(&result.diagnostics), "E021");
}

#[test]
#[ignore = "uses base functions (Flow.len) not in standalone compiler"]
fn synthetic_flow_import_with_except_does_not_require_file_module() {
    let root = temp_root("flow_import");
    let entry_path = root.join("Main.flx");
    let entry_source = "import Flow except [print]\nFlow.len([1, 2, 3]);";
    write_file(&entry_path, entry_source);
    let (program, interner) = parse_program(entry_source);

    compile_with_graph(&entry_path, &program, interner, &[root])
        .expect("expected synthetic Flow import to compile without filesystem module");
}
