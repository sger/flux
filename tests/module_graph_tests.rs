use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use flux::{
    bytecode::compiler::Compiler,
    frontend::{
        diagnostic::Diagnostic,
        lexer::Lexer,
        module_graph::ModuleGraph,
        parser::Parser,
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

fn parse_program(source: &str) -> Program {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    program
}

fn first_code(diags: Vec<flux::frontend::diagnostic::Diagnostic>) -> String {
    diags
        .first()
        .and_then(|d| d.code.clone())
        .unwrap_or_default()
}

fn compile_with_graph(
    entry_path: &Path,
    entry_program: &Program,
    roots: &[PathBuf],
) -> Result<(), Vec<Diagnostic>> {
    let graph = ModuleGraph::build_with_entry_and_roots(entry_path, entry_program, roots)?;
    let mut compiler = Compiler::new_with_file_path(entry_path.display().to_string());
    let mut errors = Vec::new();
    for node in graph.topo_order() {
        compiler.set_file_path(node.path.to_string_lossy().to_string());
        if let Err(mut diags) = compiler.compile(&node.program) {
            errors.append(&mut diags);
            break;
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
    let program = parse_program(entry_source);

    let err = ModuleGraph::build_with_entry_and_roots(&entry_path, &program, &[root])
        .expect_err("expected script import error");
    assert_eq!(first_code(err), "E036");
}

#[test]
fn module_path_mismatch_is_error() {
    let root = temp_root("path_mismatch");
    let module_path = root.join("Data").join("List.flx");
    write_file(
        &module_path,
        "module Data.Other { fun value() { 1; } }",
    );

    let entry_path = root.join("Main.flx");
    let entry_source = "import Data.List\n1;";
    write_file(&entry_path, entry_source);
    let program = parse_program(entry_source);

    let err = ModuleGraph::build_with_entry_and_roots(&entry_path, &program, &[root])
        .expect_err("expected module path mismatch error");
    assert_eq!(first_code(err), "E038");
}

#[test]
fn module_file_with_script_code_is_error() {
    let root = temp_root("module_script");
    let module_path = root.join("Mixed.flx");
    write_file(
        &module_path,
        "module Mixed { fun value() { 1; } }\n1;",
    );

    let entry_path = root.join("Main.flx");
    let entry_source = "import Mixed\n1;";
    write_file(&entry_path, entry_source);
    let program = parse_program(entry_source);

    let err = ModuleGraph::build_with_entry_and_roots(&entry_path, &program, &[root])
        .expect_err("expected mixed module/script error");
    assert_eq!(first_code(err), "E037");
}

#[test]
fn alias_import_compiles() {
    let root = temp_root("alias_import");
    let module_path = root.join("Data").join("MyFile.flx");
    write_file(
        &module_path,
        "module Data.MyFile { fun value() { 1; } }",
    );

    let entry_path = root.join("Main.flx");
    let entry_source = "import Data.MyFile as Alias\nAlias.value();";
    write_file(&entry_path, entry_source);
    let program = parse_program(entry_source);

    compile_with_graph(&entry_path, &program, &[root])
        .expect("expected alias import to compile");
}
