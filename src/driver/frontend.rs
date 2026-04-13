use std::path::{Path, PathBuf};

use crate::syntax::{
    Identifier, interner::Interner, lexer::Lexer, module_graph::ModuleGraph, parser::Parser,
    program::Program, statement::Statement,
};

const FLOW_MODULES: &[(&str, &str)] = &[
    ("Flow.Option", "Option.flx"),
    ("Flow.List", "List.flx"),
    ("Flow.String", "String.flx"),
    ("Flow.Numeric", "Numeric.flx"),
    ("Flow.IO", "IO.flx"),
    ("Flow.Assert", "Assert.flx"),
];

/// Prepends default `Flow.*` imports that exist on disk and are not already
/// present in the parsed entry program.
///
/// This keeps driver entrypoints aligned with the standard library surface
/// expected by CLI execution while preserving the caller's parser interner.
pub(crate) fn inject_flow_modules(program: &mut Program, parser: &mut Parser) {
    let flow_dir = Path::new("lib").join("Flow");
    if !flow_dir.exists() {
        return;
    }

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
    for &(module_name, file_name) in FLOW_MODULES {
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

    let flow_source = imports.join("\n");
    let main_interner = parser.take_interner();
    let flow_lexer = Lexer::new_with_interner(&flow_source, main_interner);
    let mut flow_parser = Parser::new(flow_lexer);
    let flow_program = flow_parser.parse_program();

    let enriched_interner = flow_parser.take_interner();
    parser.restore_interner(enriched_interner);

    let mut new_statements = flow_program.statements;
    new_statements.append(&mut program.statements);
    program.statements = new_statements;
}

/// Collects module resolution roots for an entry file.
///
/// Starts with any explicit roots supplied by the caller. Unless `roots_only`
/// is set, this also includes the entry file's parent directory plus local
/// `src/` and `lib/` directories when they exist.
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

/// Returns the first declared module name in the program together with its
/// interned identifier.
///
/// The string form is useful for diagnostics and cache metadata, while the
/// symbol preserves the original interned identity for downstream consumers.
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

/// Parses an entry file and builds a module graph suitable for cache lookups.
///
/// The graph is only returned when parsing, Flow import injection, and module
/// graph construction complete without module-graph diagnostics.
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
    inject_flow_modules(&mut program, &mut parser);
    let interner = parser.take_interner();
    let graph_result =
        ModuleGraph::build_with_entry_and_roots(entry_path, &program, interner, &roots);
    if !graph_result.diagnostics.is_empty() {
        return Err("module graph diagnostics present".to_string());
    }
    Ok(graph_result.graph)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::{
        driver::frontend::{
            collect_roots, extract_module_name_and_sym, inject_flow_modules,
            load_module_graph_for_cache_info,
        },
        syntax::{lexer::Lexer, parser::Parser, program::Program, statement::Statement},
    };

    fn parse_program(source: &str) -> (Program, Parser) {
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        (program, parser)
    }

    fn import_names(program: &Program, parser: &Parser) -> Vec<String> {
        let interner = parser.interner();
        program
            .statements
            .iter()
            .filter_map(|stmt| match stmt {
                Statement::Import { name, .. } => Some(interner.resolve(*name).to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn inject_flow_modules_prepends_missing_flow_imports() {
        let (mut program, mut parser) = parse_program("let answer = 42\n");

        inject_flow_modules(&mut program, &mut parser);

        let imports = import_names(&program, &parser);
        assert_eq!(imports.len(), 6);
        assert_eq!(
            imports,
            vec![
                "Flow.Option",
                "Flow.List",
                "Flow.String",
                "Flow.Numeric",
                "Flow.IO",
                "Flow.Assert"
            ]
        );
        assert!(matches!(
            program.statements.last(),
            Some(Statement::Let { .. })
        ));
    }

    #[test]
    fn inject_flow_modules_skips_existing_imports() {
        let source = "import Flow.List exposing (..)\nlet answer = 42\n";
        let (mut program, mut parser) = parse_program(source);

        inject_flow_modules(&mut program, &mut parser);

        let imports = import_names(&program, &parser);
        assert_eq!(
            imports,
            vec![
                "Flow.Option",
                "Flow.String",
                "Flow.Numeric",
                "Flow.IO",
                "Flow.Assert",
                "Flow.List",
            ]
        );
        assert_eq!(
            imports.iter().filter(|name| *name == "Flow.List").count(),
            1
        );
    }

    #[test]
    fn collect_roots_appends_entry_parent_and_project_dirs() {
        let extra = vec![PathBuf::from("tests/flux")];

        let roots = collect_roots(Path::new("examples/basics/arithmetic.flx"), &extra, false);

        assert_eq!(roots[0], PathBuf::from("tests/flux"));
        assert!(roots.contains(&PathBuf::from("examples/basics")));
        assert!(roots.contains(&PathBuf::from("src")));
        assert!(roots.contains(&PathBuf::from("lib")));
    }

    #[test]
    fn collect_roots_respects_roots_only() {
        let extra = vec![PathBuf::from("tests/parity")];

        let roots = collect_roots(Path::new("examples/basics/arithmetic.flx"), &extra, true);

        assert_eq!(roots, extra);
    }

    #[test]
    fn extract_module_name_and_sym_returns_first_module() {
        let source = "module Demo { let value = 1 }\n";
        let (program, mut parser) = parse_program(source);
        let interner = parser.take_interner();

        let (module_name, sym) =
            extract_module_name_and_sym(&program, &interner).expect("expected module");

        assert_eq!(module_name, "Demo");
        assert_eq!(interner.resolve(sym), "Demo");
    }

    #[test]
    fn extract_module_name_and_sym_returns_none_without_module() {
        let (program, mut parser) = parse_program("let value = 1\n");
        let interner = parser.take_interner();

        assert_eq!(extract_module_name_and_sym(&program, &interner), None);
    }

    #[test]
    fn load_module_graph_for_cache_info_builds_graph_for_valid_entry() {
        let graph = load_module_graph_for_cache_info("examples/basics/arithmetic.flx", &[])
            .expect("expected module graph");

        assert!(graph.module_count() >= 1);
        assert!(graph.entry_node().is_some());
    }
}
