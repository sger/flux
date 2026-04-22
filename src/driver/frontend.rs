//! Shared frontend helpers used by driver entrypoints before backend dispatch.

use std::path::{Path, PathBuf};

use crate::syntax::{
    Identifier, interner::Interner, lexer::Lexer, module_graph::ModuleGraph, parser::Parser,
    program::Program, statement::Statement,
};

const FLOW_PRELUDE_MODULES: &[(&str, &str)] = &[
    ("Flow.Option", "Option.flx"),
    ("Flow.Either", "Either.flx"),
    ("Flow.List", "List.flx"),
    ("Flow.String", "String.flx"),
    ("Flow.Numeric", "Numeric.flx"),
    ("Flow.Math", "Math.flx"),
    ("Flow.IO", "IO.flx"),
    ("Flow.Assert", "Assert.flx"),
];

/// Injects Flow prelude imports for standard modules that are present but not explicitly imported.
pub(crate) fn inject_flow_prelude(program: &mut Program, parser: &mut Parser, native_mode: bool) {
    let flow_dir = Path::new("lib").join("Flow");
    if !flow_dir.exists() {
        return;
    }

    let _ = native_mode;
    let interner = parser.interner();
    let existing_imports: Vec<String> = program
        .statements
        .iter()
        .filter_map(|stmt| {
            if let Statement::Import { name, .. } = stmt {
                interner.try_resolve(*name).map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();

    let mut imports = Vec::new();
    for &(module_name, file_name) in FLOW_PRELUDE_MODULES {
        if existing_imports.iter().any(|s| s == module_name) {
            continue;
        }
        if !flow_dir.join(file_name).exists() {
            continue;
        }
        imports.push(format!("import {module_name} exposing (..)"));
    }

    if imports.is_empty() {
        return;
    }

    let prelude_source = imports.join("\n");
    let main_interner = parser.take_interner();
    let prelude_lexer = Lexer::new_with_interner(&prelude_source, main_interner);
    let mut prelude_parser = Parser::new(prelude_lexer);
    let prelude_program = prelude_parser.parse_program();

    let enriched_interner = prelude_parser.take_interner();
    parser.restore_interner(enriched_interner);

    let mut new_statements = prelude_program.statements;
    new_statements.append(&mut program.statements);
    program.statements = new_statements;
}

/// Collects module search roots for the given entry file.
pub(crate) fn collect_roots(
    entry_path: &Path,
    extra_roots: &[PathBuf],
    roots_only: bool,
) -> Vec<PathBuf> {
    let mut roots = extra_roots.to_vec();
    if !roots_only {
        if let Some(parent) = entry_path.parent() {
            roots.push(parent.to_path_buf());
        }
        let project_src = Path::new("src");
        if project_src.exists() {
            roots.push(project_src.to_path_buf());
        }
        let project_lib = Path::new("lib");
        if project_lib.exists() {
            roots.push(project_lib.to_path_buf());
        }
    }
    roots
}

/// Extracts the declared module name from a parsed program.
pub(crate) fn extract_module_name_and_sym(
    program: &Program,
    interner: &Interner,
) -> Option<(String, Identifier)> {
    for stmt in &program.statements {
        if let Statement::Module { name, .. } = stmt {
            return Some((interner.resolve(*name).to_string(), *name));
        }
    }
    None
}

/// Loads a module graph for cache inspection commands and rejects graphs with diagnostics.
pub(crate) fn load_module_graph_for_cache_info(
    path: &str,
    extra_roots: &[PathBuf],
) -> Result<ModuleGraph, String> {
    let source = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    let entry_path = Path::new(path);
    let roots = collect_roots(entry_path, extra_roots, false);
    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let mut program = parser.parse_program();
    inject_flow_prelude(&mut program, &mut parser, false);
    let interner = parser.take_interner();
    let graph_result =
        ModuleGraph::build_with_entry_and_roots(entry_path, &program, interner, &roots);
    if !graph_result.diagnostics.is_empty() {
        return Err("module graph diagnostics present".to_string());
    }
    Ok(graph_result.graph)
}
