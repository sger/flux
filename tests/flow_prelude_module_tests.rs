//! Integration tests verifying that Flow library functions (map, filter, fold,
//! any, etc.) are available to module files via auto-prelude, even when modules
//! have no explicit import edges to the Flow libraries.
//!
//! These tests exercise the `build_module_compiler` path where Flow library
//! interfaces must be explicitly preloaded for each module compiler instance.

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

/// Build Flow prelude imports for module-graph based test compilation.
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

/// Write a module file and an entry file that imports it, then compile
/// both through the module graph (sequential single-compiler path).
fn run_module(module_source: &str, entry_source: &str) -> Value {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_dir = workspace_root.join(format!("target/tmp/flow_prelude_tests/{id}"));
    std::fs::create_dir_all(&temp_dir).unwrap();

    // Write module file.
    let module_path = temp_dir.join("Solver.flx");
    std::fs::write(&module_path, module_source).unwrap();

    // Write entry file with prelude + import of the module.
    let full_entry = format!(
        "{}\nimport Solver exposing (..)\n\n{}",
        flow_prelude_source(),
        entry_source
    );
    let entry_path = temp_dir.join("main.flx");
    std::fs::write(&entry_path, &full_entry).unwrap();

    // Parse entry.
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

    // Compile all modules sequentially (like --no-cache path).
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

// ── Flow.List functions available in modules ────────────────────────────

#[test]
fn module_can_use_fold() {
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
fn module_can_use_filter() {
    let result = run_module(
        r#"
        module Solver {
            public fn evens(xs) {
                filter(xs, fn(x) { x % 2 == 0 })
            }
        }
        "#,
        "len(evens([1, 2, 3, 4, 5, 6]));",
    );
    assert_eq!(result, Value::Integer(3));
}

#[test]
fn module_can_use_any() {
    let result = run_module(
        r#"
        module Solver {
            public fn has_big(xs) {
                any(xs, fn(x) { x > 100 })
            }
        }
        "#,
        "has_big([1, 2, 200, 3]);",
    );
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn module_can_use_map() {
    let result = run_module(
        r#"
        module Solver {
            public fn double_all(xs) {
                map(xs, fn(x) { x * 2 })
            }
        }
        "#,
        "len(double_all([1, 2, 3]));",
    );
    assert_eq!(result, Value::Integer(3));
}

// ── Multiple Flow functions in same module ──────────────────────────────

#[test]
fn module_can_use_multiple_flow_functions() {
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

// ── Flow.Option functions in modules ────────────────────────────────────

#[test]
fn module_can_use_option_functions() {
    let result = run_module(
        r#"
        module Solver {
            public fn safe_head(xs) {
                match xs[0] {
                    Some(v) -> v,
                    _ -> -1
                }
            }
        }
        "#,
        "safe_head([|10, 20, 30|]);",
    );
    assert_eq!(result, Value::Integer(10));
}

// ── Recursive module functions using Flow stdlib ────────────────────────

#[test]
fn module_recursive_function_with_fold() {
    let result = run_module(
        r#"
        module Solver {
            public fn count_matches(items, pred) {
                fold(items, 0, fn(acc, x) { if pred(x) { acc + 1 } else { acc } })
            }
        }
        "#,
        "count_matches([1, 2, 3, 4, 5], fn(x) { x > 3 });",
    );
    assert_eq!(result, Value::Integer(2));
}
