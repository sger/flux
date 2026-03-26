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
        DEFAULT_MAX_ERRORS, Diagnostic, DiagnosticsAggregator, Severity,
        quality::module_skipped_note,
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
            all_diagnostics.push(module_skipped_note(
                node.path.to_string_lossy().to_string(),
                node.path.to_string_lossy().to_string(),
                dep.name.clone(),
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
        "module Dups { fn value() { 1; } fn value() { 2; } }",
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
    write_file(&root.join("Alpha.flx"), "module Alpha { fn a() { 1; }");
    write_file(&root.join("Beta.flx"), "module Beta { fn b() { 2; }");
    write_file(&root.join("Gamma.flx"), "module Gamma { fn c() { 3; }");

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
    write_file(&root.join("Broken.flx"), "module Broken { fn a() { 1; }");
    write_file(
        &root.join("Valid.flx"),
        "module Valid { fn x() { 1; } fn x() { 2; } }",
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
            &format!("module {} {{ fn f() {{ 1; }}", mod_name),
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

#[test]
fn parser_example_max_error_cap_enforcement() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let root = temp_root("parser_example_cap");
    let entry = root.join("Main.flx");
    let source =
        include_str!("../examples/parser_errors/max_errors_many_functions_missing_brace.flx");
    write_file(&entry, source);

    let (diags, rendered) = run_unified_pipeline(&entry, source, &[root], DEFAULT_MAX_ERRORS);

    let error_count = diags
        .iter()
        .filter(|d| d.severity() == Severity::Error)
        .count();
    assert!(
        error_count > DEFAULT_MAX_ERRORS,
        "expected >50 parser errors, got {}",
        error_count
    );
    assert!(
        rendered.contains("not shown"),
        "expected truncation note in parser output, got:\n{}",
        rendered
    );
    assert_eq!(
        rendered.matches("error[E034]").count(),
        DEFAULT_MAX_ERRORS,
        "expected exactly {} rendered parser errors before truncation, got:\n{}",
        DEFAULT_MAX_ERRORS,
        rendered
    );
}

#[test]
fn effect_example_max_error_cap_enforcement() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let root = temp_root("effect_example_cap");
    let entry = root.join("Main.flx");
    let source =
        include_str!("../examples/type_system/failing/205_effect_max_errors_many_missing_io.flx");
    write_file(&entry, source);

    let (diags, rendered) = run_unified_pipeline(&entry, source, &[root], DEFAULT_MAX_ERRORS);

    let e400_count = diags
        .iter()
        .filter(|d| d.severity() == Severity::Error && d.code() == Some("E400"))
        .count();
    assert!(
        e400_count > DEFAULT_MAX_ERRORS,
        "expected >50 independent E400 diagnostics, got {}: {:?}",
        e400_count,
        diags.iter().filter_map(|d| d.code()).collect::<Vec<_>>()
    );
    assert!(
        rendered.contains("not shown"),
        "expected truncation note in effect output, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("suppressed by stage filtering"),
        "did not expect stage filtering suppression for pure effect corpus, got:\n{}",
        rendered
    );
    assert_eq!(
        rendered.matches("error[E400]").count(),
        DEFAULT_MAX_ERRORS,
        "expected exactly {} rendered E400 diagnostics before truncation, got:\n{}",
        DEFAULT_MAX_ERRORS,
        rendered
    );
}

// ---------------------------------------------------------------------------
// Test 6: PASS 2 multi-error continuation keeps independent compiler errors
// in deterministic source order within one module file.
// ---------------------------------------------------------------------------
#[test]
fn pass2_multi_error_continuation_ordering() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let root = temp_root("pass2_multi_error_continuation");
    let entry = root.join("Main.flx");
    let source = include_str!("../examples/type_system/failing/99_multi_error_continuation.flx");
    write_file(&entry, source);

    let (diags, rendered) = run_unified_pipeline(&entry, source, &[root], DEFAULT_MAX_ERRORS);

    let error_codes: Vec<_> = diags
        .iter()
        .filter(|d| d.severity() == Severity::Error)
        .filter_map(|d| d.code())
        .collect();
    assert!(
        error_codes.contains(&"E002"),
        "expected E002 in unified diagnostics, got {:?}",
        error_codes
    );
    assert!(
        error_codes.contains(&"E300"),
        "expected E300 in unified diagnostics, got {:?}",
        error_codes
    );

    let e002_idx = rendered
        .find("error[E002]")
        .expect("expected rendered E002 in unified output");
    let e300_idx = rendered
        .find("error[E300]")
        .expect("expected rendered E300 in unified output");
    assert!(
        e002_idx < e300_idx,
        "expected deterministic source-order diagnostics (E002 before E300), got:\n{}",
        rendered
    );
}

#[test]
fn adversarial_compiler_fixture_reports_multiple_independent_errors() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let root = temp_root("adversarial_compiler_multi");
    let entry = root.join("Main.flx");
    let source =
        include_str!("../examples/compiler_errors/adversarial/multi_independent_errors.flx");
    write_file(&entry, source);

    let (diags, rendered) = run_unified_pipeline(&entry, source, &[root], DEFAULT_MAX_ERRORS);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity() == Severity::Error)
        .collect();

    assert!(
        errors.iter().any(|d| d.code() == Some("E300")),
        "expected an E300 type mismatch, got {:?}",
        errors.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
    assert!(
        errors.iter().any(|d| d.code() == Some("E056")),
        "expected an E056 arity mismatch, got {:?}",
        errors.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
    assert!(
        errors.iter().any(|d| d.code() == Some("E400")),
        "expected an E400 effect diagnostic before rendering-stage filtering, got {:?}",
        errors.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
    assert!(rendered.contains("error[E300]: Annotation Type Mismatch"));
    assert!(rendered.contains("error[E056]: Wrong Number Of Arguments"));
    assert!(
        !rendered.contains("error[E400]: Missing Ambient Effect"),
        "expected same-module effect diagnostic to stay suppressed in rendered output:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Downstream Errors Suppressed"),
        "expected same-module suppression note in rendered output:\n{}",
        rendered
    );
}

#[test]
fn stage_filtering_suppresses_effects_but_all_errors_preserves_them() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let root = temp_root("adversarial_compiler_stage_filter");
    write_file(
        &root.join("TypeBroken.flx"),
        include_str!("../examples/compiler_errors/adversarial/stage_all_errors/TypeBroken.flx"),
    );
    write_file(
        &root.join("EffectBroken.flx"),
        include_str!("../examples/compiler_errors/adversarial/stage_all_errors/EffectBroken.flx"),
    );
    let entry = root.join("Main.flx");
    let source = include_str!("../examples/compiler_errors/adversarial/stage_all_errors/Main.flx");
    write_file(&entry, source);

    let (diags, rendered) = run_unified_pipeline(&entry, source, &[root], DEFAULT_MAX_ERRORS);
    assert!(
        diags.iter().any(|d| d.code() == Some("E300")),
        "expected upstream type error in raw diagnostics"
    );
    assert!(
        diags.iter().any(|d| d.code() == Some("E400")),
        "expected downstream effect error in raw diagnostics"
    );
    assert!(rendered.contains("error[E300]: Annotation Type Mismatch"));
    assert!(rendered.contains("error[E400]: Missing Ambient Effect"));
    assert!(
        !rendered.contains("Downstream Errors Suppressed"),
        "did not expect cross-module suppression in rendered output:\n{}",
        rendered
    );
}

#[test]
fn common_dev_mistakes_graph_reports_broad_compiler_errors() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let root = temp_root("common_dev_mistakes");
    write_file(
        &root.join("Main.flx"),
        include_str!("../examples/compiler_errors/adversarial/common_dev_mistakes/Main.flx"),
    );
    write_file(
        &root.join("HiddenApi.flx"),
        include_str!("../examples/compiler_errors/adversarial/common_dev_mistakes/HiddenApi.flx"),
    );
    write_file(
        &root.join("MissingPublicAccess.flx"),
        include_str!(
            "../examples/compiler_errors/adversarial/common_dev_mistakes/MissingPublicAccess.flx"
        ),
    );
    write_file(
        &root.join("PrivateMemberAccess.flx"),
        include_str!(
            "../examples/compiler_errors/adversarial/common_dev_mistakes/PrivateMemberAccess.flx"
        ),
    );
    write_file(
        &root.join("CollectionsConfusion.flx"),
        include_str!(
            "../examples/compiler_errors/adversarial/common_dev_mistakes/CollectionsConfusion.flx"
        ),
    );
    write_file(
        &root.join("UnknownEffectTypo.flx"),
        include_str!(
            "../examples/compiler_errors/adversarial/common_dev_mistakes/UnknownEffectTypo.flx"
        ),
    );
    write_file(
        &root.join("UnknownEffectOpTypo.flx"),
        include_str!(
            "../examples/compiler_errors/adversarial/common_dev_mistakes/UnknownEffectOpTypo.flx"
        ),
    );
    write_file(
        &root.join("UnknownBaseMemberTypo.flx"),
        include_str!(
            "../examples/compiler_errors/adversarial/common_dev_mistakes/UnknownBaseMemberTypo.flx"
        ),
    );
    write_file(
        &root.join("UnknownIdentifierTypo.flx"),
        include_str!(
            "../examples/compiler_errors/adversarial/common_dev_mistakes/UnknownIdentifierTypo.flx"
        ),
    );
    write_file(
        &root.join("WrongArity.flx"),
        include_str!("../examples/compiler_errors/adversarial/common_dev_mistakes/WrongArity.flx"),
    );

    let entry = root.join("Main.flx");
    let source =
        include_str!("../examples/compiler_errors/adversarial/common_dev_mistakes/Main.flx");
    let (diags, rendered) = run_unified_pipeline(&entry, source, &[root], DEFAULT_MAX_ERRORS);

    assert!(
        diags.iter().any(|d| d.code() == Some("E011")),
        "expected a non-public/private member access diagnostic, got {:?}",
        diags.iter().filter_map(|d| d.code()).collect::<Vec<_>>()
    );
    assert!(
        diags.iter().any(|d| d.code() == Some("E300")),
        "expected a list/array confusion type mismatch, got {:?}",
        diags.iter().filter_map(|d| d.code()).collect::<Vec<_>>()
    );
    assert!(
        diags.iter().any(|d| d.code() == Some("E407")),
        "expected an unknown effect typo diagnostic, got {:?}",
        diags.iter().filter_map(|d| d.code()).collect::<Vec<_>>()
    );
    assert!(
        diags
            .iter()
            .any(|d| matches!(d.code(), Some("E404") | Some("E080") | Some("E013"))),
        "expected an unknown operation/member typo diagnostic, got {:?}",
        diags.iter().filter_map(|d| d.code()).collect::<Vec<_>>()
    );
    assert!(
        diags.iter().any(|d| d.code() == Some("E004")),
        "expected an undefined-identifier typo diagnostic, got {:?}",
        diags.iter().filter_map(|d| d.code()).collect::<Vec<_>>()
    );
    assert!(
        diags.iter().any(|d| d.code() == Some("E056")),
        "expected a wrong-arity diagnostic, got {:?}",
        diags.iter().filter_map(|d| d.code()).collect::<Vec<_>>()
    );
    assert!(
        rendered.contains("Private Member"),
        "expected contextual E011 title in rendered output, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Argument Type Mismatch"),
        "expected contextual E300 title in rendered output, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Wrong Number Of Arguments"),
        "expected wrong-arity title in rendered output, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Undefined Variable"),
        "expected undefined-variable typo title in rendered output, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Undefined Variable"),
        "expected undefined-variable diagnostic in rendered output, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("Downstream Errors Suppressed"),
        "did not expect cross-module suppression note in rendered output, got:\n{}",
        rendered
    );
}

// ---------------------------------------------------------------------------
// Test 7: Stable ordering -- same input twice -> identical output
// ---------------------------------------------------------------------------
#[test]
fn stable_ordering() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let root = temp_root("stable_order");
    write_file(
        &root.join("A.flx"),
        "module A { fn x() { 1; } fn x() { 2; } }",
    );
    write_file(
        &root.join("B.flx"),
        "module B { fn y() { 1; } fn y() { 2; } }",
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
