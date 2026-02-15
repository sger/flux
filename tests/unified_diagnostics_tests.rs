mod diagnostics_env;

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use flux::{
    bytecode::compiler::Compiler,
    diagnostics::{
        DEFAULT_MAX_ERRORS, Diagnostic, DiagnosticsAggregator, Severity, position::Span,
    },
    syntax::{lexer::Lexer, module_graph::ModuleGraph, parser::Parser},
};

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut root = std::env::temp_dir();
    root.push(format!("flux_unified_diag_tests_{}_{}", label, nanos));
    fs::create_dir_all(&root).expect("create temp root");
    root
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write file");
}

/// Runs the full unified diagnostic pipeline (parse -> graph -> compile) as
/// main.rs does, collecting all diagnostics into a single pool. Returns the
/// pool and the aggregated report string.
fn run_unified_pipeline(
    entry_path: &Path,
    entry_source: &str,
    roots: &[PathBuf],
    max_errors: usize,
) -> (Vec<Diagnostic>, String) {
    let lexer = Lexer::new(entry_source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();

    let mut all_diagnostics: Vec<Diagnostic> = Vec::new();
    let entry_has_errors = !parser.errors.is_empty();
    if entry_has_errors {
        for diag in &mut parser.errors {
            if diag.file().is_none() {
                diag.set_file(entry_path.display().to_string());
            }
        }
        all_diagnostics.append(&mut parser.errors);
    }

    let interner = parser.take_interner();
    let graph_result =
        ModuleGraph::build_with_entry_and_roots(entry_path, &program, interner, roots);
    all_diagnostics.extend(graph_result.diagnostics);

    let mut failed: HashSet<PathBuf> = graph_result.failed_modules;
    if entry_has_errors && let Ok(canon) = std::fs::canonicalize(entry_path) {
        failed.insert(canon);
    }

    let is_multimodule = graph_result.graph.module_count() > 1;
    let graph = graph_result.graph;

    let mut compiler =
        Compiler::new_with_interner(entry_path.display().to_string(), graph_result.interner);
    let entry_canonical = std::fs::canonicalize(entry_path).ok();
    for node in graph.topo_order() {
        if entry_has_errors
            && let Some(ref canon) = entry_canonical
            && &node.path == canon
        {
            continue;
        }
        let failed_dep = node
            .imports
            .iter()
            .find(|e| failed.contains(&e.target_path));
        if let Some(dep) = failed_dep {
            failed.insert(node.path.clone());
            all_diagnostics.push(Diagnostic::make_note(
                "MODULE SKIPPED",
                format!(
                    "Module `{}` was skipped because its dependency `{}` has errors.",
                    node.path.to_string_lossy(),
                    dep.name,
                ),
                node.path.to_string_lossy().to_string(),
                Span::default(),
            ));
            continue;
        }
        compiler.set_file_path(node.path.to_string_lossy().to_string());
        if let Err(mut diags) = compiler.compile(&node.program) {
            for diag in &mut diags {
                if diag.file().is_none() {
                    diag.set_file(node.path.to_string_lossy().to_string());
                }
            }
            all_diagnostics.append(&mut diags);
            continue;
        }
    }

    let rendered = if all_diagnostics.is_empty() {
        String::new()
    } else {
        DiagnosticsAggregator::new(&all_diagnostics)
            .with_default_source(entry_path.display().to_string(), entry_source)
            .with_file_headers(is_multimodule)
            .with_max_errors(Some(max_errors))
            .report()
            .rendered
    };

    (all_diagnostics, rendered)
}

// ---------------------------------------------------------------------------
// Test 1: Single-file script with multiple compile errors -- all reported
// ---------------------------------------------------------------------------
#[test]
fn script_syntax_errors_reported() {
    let root = temp_root("script_syntax");
    let entry = root.join("Main.flx");
    // Two undefined variable references produce separate E004 errors.
    let source = "let x = unknown_a;\nlet y = unknown_b;\n1;";
    write_file(&entry, source);

    let (diags, rendered) = run_unified_pipeline(&entry, source, &[root], DEFAULT_MAX_ERRORS);
    let error_count = diags
        .iter()
        .filter(|d| d.severity() == Severity::Error)
        .count();
    assert!(
        error_count >= 2,
        "expected at least 2 errors, got {}: {:?}",
        error_count,
        diags
    );
    assert!(!rendered.is_empty(), "rendered output should not be empty");
}

// ---------------------------------------------------------------------------
// Test 2: Module with duplicate names -- semantic diagnostics work
// ---------------------------------------------------------------------------
#[test]
fn single_module_semantic_errors() {
    let root = temp_root("semantic_errors");
    let module_path = root.join("Dups.flx");
    write_file(
        &module_path,
        "module Dups { fun value() { 1; } fun value() { 2; } }",
    );

    let entry = root.join("Main.flx");
    let source = "import Dups\nDups.value();";
    write_file(&entry, source);

    let (diags, _rendered) = run_unified_pipeline(&entry, source, &[root], DEFAULT_MAX_ERRORS);
    assert!(!diags.is_empty(), "expected semantic diagnostics");
    let has_dup = diags.iter().any(|d| d.code() == Some("E001"));
    assert!(
        has_dup,
        "expected E001 DUPLICATE NAME, got codes: {:?}",
        diags.iter().filter_map(|d| d.code()).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Test 3: Three imported modules each with parse errors -- all 3 reported
// ---------------------------------------------------------------------------
#[test]
fn three_modules_independent_parse_errors() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let root = temp_root("three_errors");
    write_file(&root.join("Alpha.flx"), "module Alpha { fun a() { 1; }");
    write_file(&root.join("Beta.flx"), "module Beta { fun b() { 2; }");
    write_file(&root.join("Gamma.flx"), "module Gamma { fun c() { 3; }");

    let entry = root.join("Main.flx");
    let source = "import Alpha\nimport Beta\nimport Gamma\n1;";
    write_file(&entry, source);

    let (diags, rendered) = run_unified_pipeline(&entry, source, &[root], DEFAULT_MAX_ERRORS);
    let error_files: HashSet<String> = diags
        .iter()
        .filter(|d| d.severity() == Severity::Error)
        .filter_map(|d| d.file().map(String::from))
        .collect();
    assert!(
        error_files.len() >= 3,
        "expected errors from at least 3 files, got errors in {} files: {:?}",
        error_files.len(),
        error_files
    );
    assert!(
        rendered.contains("across"),
        "expected 'across N modules' in summary, got:\n{}",
        rendered
    );
}

// ---------------------------------------------------------------------------
// Test 4: Module A has parse error, module B is valid but independent.
//         B's semantic errors still checked, A's parse errors reported.
// ---------------------------------------------------------------------------
#[test]
fn mixed_graph_parse_and_semantic() {
    let root = temp_root("mixed_errors");
    write_file(&root.join("Broken.flx"), "module Broken { fun a() { 1; }");
    write_file(
        &root.join("Valid.flx"),
        "module Valid { fun x() { 1; } fun x() { 2; } }",
    );

    let entry = root.join("Main.flx");
    let source = "import Broken\nimport Valid\n1;";
    write_file(&entry, source);

    let (diags, _rendered) = run_unified_pipeline(&entry, source, &[root], DEFAULT_MAX_ERRORS);

    let has_parse_error = diags
        .iter()
        .any(|d| d.severity() == Severity::Error && d.file().is_some_and(|f| f.contains("Broken")));
    assert!(has_parse_error, "expected parse error from Broken module");

    let has_semantic_error = diags
        .iter()
        .any(|d| d.code() == Some("E001") && d.file().is_some_and(|f| f.contains("Valid")));
    assert!(
        has_semantic_error,
        "expected E001 from Valid module (no cascade from Broken). Diags: {:?}",
        diags
            .iter()
            .map(|d| format!("{:?} file={:?}", d.code(), d.file()))
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Test 5: Diagnostic cap enforcement -- generate >50 errors -> only 50 shown
//
// Module compilation stops at the first error per module body, so we create
// 60 distinct modules each with a parse error (missing closing brace). The
// graph discovers all of them, producing one E076 per module, totalling 60
// errors which exceeds the DEFAULT_MAX_ERRORS (50) cap.
// ---------------------------------------------------------------------------
#[test]
fn diagnostic_cap_enforcement() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let root = temp_root("cap_enforcement");

    let mut imports = String::new();
    for i in 0..60 {
        let mod_name = format!("Mod{}", i);
        write_file(
            &root.join(format!("{}.flx", mod_name)),
            &format!("module {} {{ fun f() {{ 1; }}", mod_name),
        );
        imports.push_str(&format!("import {}\n", mod_name));
    }
    imports.push_str("1;");

    let entry = root.join("Main.flx");
    write_file(&entry, &imports);

    let (diags, rendered) = run_unified_pipeline(&entry, &imports, &[root], DEFAULT_MAX_ERRORS);

    let error_count = diags
        .iter()
        .filter(|d| d.severity() == Severity::Error)
        .count();
    assert!(
        error_count > DEFAULT_MAX_ERRORS,
        "test setup: expected >50 errors to test cap, got {}",
        error_count
    );
    assert!(
        rendered.contains("not shown"),
        "expected truncation message in output, got:\n{}",
        rendered
    );
}

// ---------------------------------------------------------------------------
// Test 6: Stable ordering -- same input twice -> identical output
// ---------------------------------------------------------------------------
#[test]
fn stable_ordering() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let root = temp_root("stable_order");
    write_file(
        &root.join("A.flx"),
        "module A { fun x() { 1; } fun x() { 2; } }",
    );
    write_file(
        &root.join("B.flx"),
        "module B { fun y() { 1; } fun y() { 2; } }",
    );

    let entry = root.join("Main.flx");
    let source = "import A\nimport B\n1;";
    write_file(&entry, source);

    let (_, rendered1) = run_unified_pipeline(
        &entry,
        source,
        std::slice::from_ref(&root),
        DEFAULT_MAX_ERRORS,
    );
    let (_, rendered2) = run_unified_pipeline(&entry, source, &[root], DEFAULT_MAX_ERRORS);

    assert_eq!(
        rendered1, rendered2,
        "output should be deterministic across runs"
    );
    assert!(!rendered1.is_empty(), "expected diagnostics output");
}
