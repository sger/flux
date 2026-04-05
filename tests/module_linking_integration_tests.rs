//! Integration tests verifying that the module linker correctly patches constant
//! indices in superinstruction opcodes (OpConstantAdd, OpGetLocalIsAdt, etc.)
//! when modules are linked with a non-zero constant base.
//!
//! These tests caught bugs where the linker skipped rebasing for new opcodes,
//! causing the VM to index into wrong constants (e.g., adding Int + String).

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

/// Write a module file and an entry file that imports it, compile through
/// the module graph, execute in the VM, and return the last popped value.
fn run_module(module_source: &str, entry_source: &str) -> Value {
    run_named_module("Lib", module_source, entry_source)
}

fn run_named_module(module_file_name: &str, module_source: &str, entry_source: &str) -> Value {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_dir = workspace_root.join(format!("target/tmp/module_linking_tests/{id}"));
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
            let source =
                std::fs::read_to_string(&node.path).unwrap_or_else(|_| full_entry.clone());
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

/// Write two module files plus an entry, compile, and run.
fn run_two_modules(
    mod_a_name: &str,
    mod_a_source: &str,
    mod_b_name: &str,
    mod_b_source: &str,
    entry_source: &str,
) -> Value {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_dir = workspace_root.join(format!("target/tmp/module_linking_tests/{id}"));
    std::fs::create_dir_all(&temp_dir).unwrap();

    std::fs::write(
        temp_dir.join(format!("{mod_a_name}.flx")),
        mod_a_source,
    )
    .unwrap();
    std::fs::write(
        temp_dir.join(format!("{mod_b_name}.flx")),
        mod_b_source,
    )
    .unwrap();

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
            let source =
                std::fs::read_to_string(&node.path).unwrap_or_else(|_| full_entry.clone());
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

// ── OpConstantAdd rebasing ──────────────────────────────────────────────

#[test]
fn constant_add_after_linking() {
    // Module defines a function using x + 1 (OpConstantAdd).
    // Entry has its own constants, shifting the module's constant base.
    let result = run_module(
        r#"
        module Lib {
            public fn inc(x) { x + 1 }
        }
        "#,
        r#"
        fn main() {
            let a = "pad1"
            let b = "pad2"
            let c = "pad3"
            let d = "pad4"
            let e = "pad5"
            inc(41)
        }
        "#,
    );
    assert_eq!(result, Value::Integer(42));
}

#[test]
fn constant_add_with_string_constant_base() {
    // Module has string constants; entry does integer arithmetic.
    // If the linker fails to rebase OpConstantAdd, the VM would try to
    // add an Int with a String constant.
    let result = run_module(
        r#"
        module Lib {
            fn greeting() { "hello" }
            fn separator() { " " }
            fn suffix() { "world" }
            public fn next(x) { x + 1 }
        }
        "#,
        "next(99);",
    );
    assert_eq!(result, Value::Integer(100));
}

// ── ADT pattern matching after linking ──────────────────────────────────

#[test]
fn adt_pattern_match_after_linking() {
    // Module defines an ADT and pattern-matches on it.
    // OpIsAdt constant indices must be rebased after linking.
    let result = run_module(
        r#"
        module Lib {
            data Shape {
                Circle(Int),
                Rect(Int, Int),
            }

            public fn area(s) {
                match s {
                    Circle(r) -> r * r,
                    Rect(w, h) -> w * h,
                    _ -> 0,
                }
            }

            public fn make_rect(w, h) { Rect(w, h) }
            public fn make_circle(r) { Circle(r) }
        }
        "#,
        r#"
        fn main() {
            let a = area(make_circle(5))
            let b = area(make_rect(3, 4))
            a + b
        }
        "#,
    );
    assert_eq!(result, Value::Integer(25 + 12));
}

// ── Recursive function with OpConstantAdd across modules ────────────────

#[test]
fn recursive_function_with_constant_add_across_modules() {
    // The day07 bug pattern: recursive find(s, c + 1) in a module.
    // The c + 1 uses OpConstantAdd. After linking, the constant index
    // must be rebased or the VM adds Int + wrong-type.
    let result = run_module(
        r#"
        module Lib {
            public fn find_char(s, target, c) {
                if c >= len(s) {
                    -1
                } else if substring(s, c, c + 1) == target {
                    c
                } else {
                    find_char(s, target, c + 1)
                }
            }
        }
        "#,
        r#"find_char("hello world", "w", 0);"#,
    );
    assert_eq!(result, Value::Integer(6));
}

// ── Multiple modules chained ────────────────────────────────────────────

#[test]
fn multiple_modules_chained_linking() {
    // Three modules: Utils defines helpers, Solver imports Utils,
    // entry imports Solver. Constants are rebased twice.
    let result = run_two_modules(
        "Utils",
        r#"
        module Utils {
            public fn safe_get(arr, idx) {
                match arr[idx] {
                    Some(v) -> v,
                    _ -> 0,
                }
            }
        }
        "#,
        "Solver",
        r#"
        import Utils exposing (..)

        module Solver {
            public fn sum_first_n(arr, n) {
                if n <= 0 {
                    0
                } else {
                    Utils.safe_get(arr, n - 1) + sum_first_n(arr, n - 1)
                }
            }
        }
        "#,
        r#"sum_first_n([|10, 20, 30, 40|], 3);"#,
    );
    assert_eq!(result, Value::Integer(60));
}

// ── Closure across module boundary ──────────────────────────────────────

#[test]
fn closure_across_module_boundary() {
    // Module returns a closure. OpClosure constant indices must be
    // rebased after linking.
    let result = run_module(
        r#"
        module Lib {
            public fn make_adder(n) {
                fn(x) { x + n }
            }
        }
        "#,
        r#"
        fn main() {
            let add5 = make_adder(5)
            add5(37)
        }
        "#,
    );
    assert_eq!(result, Value::Integer(42));
}

// ── Module with many constants ──────────────────────────────────────────

#[test]
fn module_with_many_string_constants_then_arithmetic() {
    // Module has many string constants (large constant pool).
    // A numeric function at the end must still work after linking.
    let result = run_module(
        r#"
        module Lib {
            fn s0() { "aaa" }
            fn s1() { "bbb" }
            fn s2() { "ccc" }
            fn s3() { "ddd" }
            fn s4() { "eee" }
            fn s5() { "fff" }
            fn s6() { "ggg" }
            fn s7() { "hhh" }

            public fn compute(a, b) { a + b + 1 }
        }
        "#,
        "compute(20, 21);",
    );
    assert_eq!(result, Value::Integer(42));
}
