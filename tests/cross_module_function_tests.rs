//! Integration tests for cross-module function calls, recursive patterns,
//! Flow stdlib availability via auto-prelude, and peephole fusion safety
//! across module boundaries.
//!
//! These tests catch bugs where:
//! - Peephole fusion corrupts bytecode when jumps target interior operand bytes
//! - Flow library functions (fold, filter, any) are unavailable in modules
//! - Recursive tail calls with if-expression arguments fail after linking

use flux::bytecode::compiler::Compiler;
use flux::bytecode::vm::VM;
use flux::diagnostics::{DiagnosticsAggregator, render_diagnostics};
use flux::runtime::value::Value;
use flux::syntax::lexer::Lexer;
use flux::syntax::module_graph::ModuleGraph;
use flux::syntax::parser::Parser;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn flow_prelude_source() -> String {
    [
        "import Flow.Option exposing (..)",
        "import Flow.List except [concat, delete]",
        "import Flow.List as List",
        "import Flow.String exposing (..)",
        "import Flow.Numeric exposing (..)",
        "import Flow.IO exposing (..)",
        "import Flow.Assert exposing (..)",
        "",
    ]
    .join("\n")
}

fn run_module(module_source: &str, entry_source: &str) -> Value {
    run_named_module("Solver", module_source, entry_source)
}

fn run_named_module(module_file_name: &str, module_source: &str, entry_source: &str) -> Value {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_dir = workspace_root.join(format!("target/tmp/cross_module_tests/{id}"));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let module_path = temp_dir.join(format!("{module_file_name}.flx"));
    std::fs::write(&module_path, module_source).unwrap();

    let full_entry = format!(
        "{}\nimport {module_file_name} exposing (..)\n\n{entry_source}",
        flow_prelude_source(),
    );
    let entry_path = temp_dir.join("main.flx");
    std::fs::write(&entry_path, &full_entry).unwrap();

    let lexer = Lexer::new(&full_entry);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {}",
        render_diagnostics(&parser.errors, Some(&full_entry), None)
    );

    let interner = parser.take_interner();
    let roots = vec![temp_dir.clone(), workspace_root.join("lib")];
    let graph_result =
        ModuleGraph::build_with_entry_and_roots(&entry_path, &program, interner, &roots);
    if !graph_result.diagnostics.is_empty() {
        let report = DiagnosticsAggregator::new(&graph_result.diagnostics)
            .with_default_source(entry_path.to_string_lossy(), &full_entry)
            .with_file_headers(false)
            .report();
        if report.counts.errors > 0 {
            panic!("module graph errors:\n{}", report.rendered);
        }
    }

    let mut compiler = Compiler::new_with_interner(
        entry_path.to_string_lossy().to_string(),
        graph_result.interner,
    );
    for node in graph_result.graph.topo_order() {
        compiler.set_file_path(node.path.to_string_lossy().to_string());
        if let Err(diags) = compiler.compile(&node.program) {
            let source = std::fs::read_to_string(&node.path).unwrap_or_else(|_| full_entry.clone());
            panic!(
                "compile error in {}:\n{}",
                node.path.display(),
                render_diagnostics(&diags, Some(&source), None)
            );
        }
    }

    let bytecode = compiler.bytecode().clone();
    let mut vm = VM::new(bytecode);
    vm.run().unwrap_or_else(|err| panic!("VM error: {err}"));
    vm.last_popped_stack_elem()
}

fn run_two_modules(
    mod_a_name: &str,
    mod_a_source: &str,
    mod_b_name: &str,
    mod_b_source: &str,
    entry_source: &str,
) -> Value {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_dir = workspace_root.join(format!("target/tmp/cross_module_tests/{id}"));
    std::fs::create_dir_all(&temp_dir).unwrap();

    std::fs::write(temp_dir.join(format!("{mod_a_name}.flx")), mod_a_source).unwrap();
    std::fs::write(temp_dir.join(format!("{mod_b_name}.flx")), mod_b_source).unwrap();

    let full_entry = format!(
        "{}\nimport {mod_a_name} exposing (..)\nimport {mod_b_name} exposing (..)\n\n{entry_source}",
        flow_prelude_source(),
    );
    let entry_path = temp_dir.join("main.flx");
    std::fs::write(&entry_path, &full_entry).unwrap();

    let lexer = Lexer::new(&full_entry);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {}",
        render_diagnostics(&parser.errors, Some(&full_entry), None)
    );

    let interner = parser.take_interner();
    let roots = vec![temp_dir.clone(), workspace_root.join("lib")];
    let graph_result =
        ModuleGraph::build_with_entry_and_roots(&entry_path, &program, interner, &roots);
    if !graph_result.diagnostics.is_empty() {
        let report = DiagnosticsAggregator::new(&graph_result.diagnostics)
            .with_default_source(entry_path.to_string_lossy(), &full_entry)
            .with_file_headers(false)
            .report();
        if report.counts.errors > 0 {
            panic!("module graph errors:\n{}", report.rendered);
        }
    }

    let mut compiler = Compiler::new_with_interner(
        entry_path.to_string_lossy().to_string(),
        graph_result.interner,
    );
    for node in graph_result.graph.topo_order() {
        compiler.set_file_path(node.path.to_string_lossy().to_string());
        if let Err(diags) = compiler.compile(&node.program) {
            let source = std::fs::read_to_string(&node.path).unwrap_or_else(|_| full_entry.clone());
            panic!(
                "compile error in {}:\n{}",
                node.path.display(),
                render_diagnostics(&diags, Some(&source), None)
            );
        }
    }

    let bytecode = compiler.bytecode().clone();
    let mut vm = VM::new(bytecode);
    vm.run().unwrap_or_else(|err| panic!("VM error: {err}"));
    vm.last_popped_stack_elem()
}

// ── Recursive cross-module patterns ─────────────────────────────────────

#[test]
fn recursive_cross_module_with_multi_branch() {
    // The two-branch tail-call pattern that triggered the peephole fusion bug.
    // A jump target can land on an interior operand byte of a fused instruction.
    let result = run_module(
        r#"
        module Solver {
            public fn solve(items, idx, acc, checker) {
                if idx >= len(items) {
                    acc
                } else {
                    let line = match items[idx] {
                        Some(v) -> v,
                        _ -> ""
                    }
                    if len(line) == 0 {
                        solve(items, idx + 1, acc, checker)
                    } else {
                        let valid = checker(idx)
                        solve(items, idx + 1, if valid { acc + 1 } else { acc }, checker)
                    }
                }
            }
        }
        "#,
        r#"solve([|"a", "", "b", "c", ""|], 0, 0, fn(i) { i < 3 });"#,
    );
    assert_eq!(result, Value::Integer(2));
}

#[test]
fn four_arg_recursive_with_if_in_module() {
    // The accum pattern with HOF callback — tests OpConstantAdd in
    // complex recursive tail calls with if-expression arguments.
    let result = run_module(
        r#"
        module Solver {
            public fn accum(items, idx, total, f) {
                if idx >= len(items) {
                    total
                } else {
                    let ok = f(idx)
                    accum(items, idx + 1, if ok { total + 10 } else { total }, f)
                }
            }
        }
        "#,
        r#"accum([|1, 2, 3, 4, 5|], 0, 0, fn(i) { i < 3 });"#,
    );
    assert_eq!(result, Value::Integer(30));
}

// ── Flow stdlib in modules (auto-prelude) ───────────────────────────────

#[test]
fn flow_fold_in_module() {
    let result = run_module(
        r#"
        module Solver {
            public fn sum_list(xs) {
                fold(xs, 0, fn(acc, x) { acc + x })
            }
        }
        "#,
        "sum_list([1, 2, 3, 4, 5]);",
    );
    assert_eq!(result, Value::Integer(15));
}

#[test]
fn flow_filter_in_module() {
    let result = run_module(
        r#"
        module Solver {
            public fn count_evens(xs) {
                len(filter(xs, fn(x) { x % 2 == 0 }))
            }
        }
        "#,
        "count_evens([1, 2, 3, 4, 5, 6]);",
    );
    assert_eq!(result, Value::Integer(3));
}

#[test]
fn flow_any_in_module() {
    let result = run_module(
        r#"
        module Solver {
            public fn has_large(xs, threshold) {
                any(xs, fn(x) { x > threshold })
            }
        }
        "#,
        "has_large([1, 2, 200, 3], 100);",
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn flow_fold_filter_combined_in_module() {
    // Module uses both filter and fold — the full day05 pattern.
    let result = run_module(
        r#"
        module Solver {
            public fn sum_evens(xs) {
                fold(filter(xs, fn(x) { x % 2 == 0 }), 0, fn(acc, x) { acc + x })
            }
        }
        "#,
        "sum_evens([1, 2, 3, 4, 5, 6]);",
    );
    assert_eq!(result, Value::Integer(12));
}

// ── Transitive module dependencies ──────────────────────────────────────

#[test]
fn transitive_module_dependency() {
    // Module A defines a helper. Module B imports A and uses it with fold.
    // Entry imports B.
    let result = run_two_modules(
        "Helpers",
        r#"
        module Helpers {
            public fn double(x) { x * 2 }
        }
        "#,
        "Solver",
        r#"
        import Helpers exposing (..)

        module Solver {
            public fn sum_doubled(xs) {
                fold(xs, 0, fn(acc, x) { acc + Helpers.double(x) })
            }
        }
        "#,
        "sum_doubled([1, 2, 3]);",
    );
    assert_eq!(result, Value::Integer(12));
}

// ── Module with where clauses ───────────────────────────────────────────

#[test]
fn module_with_where_clauses() {
    // The day03 pattern: where bindings with fold in a module.
    let result = run_module(
        r#"
        module Solver {
            public fn count_up_to(n) {
                total
                where xs = range(0, n)
                where total = fold(xs, 0, fn(acc, x) { acc + x })
            }
        }
        "#,
        "count_up_to(5);",
    );
    // 0 + 1 + 2 + 3 + 4 = 10
    assert_eq!(result, Value::Integer(10));
}

// ── Module exporting higher-order function ──────────────────────────────

#[test]
fn module_exports_hof_used_with_map() {
    let result = run_module(
        r#"
        module Solver {
            public fn transform(xs, f) {
                map(xs, f)
            }
        }
        "#,
        "len(transform([1, 2, 3], fn(x) { x * 10 }));",
    );
    assert_eq!(result, Value::Integer(3));
}
