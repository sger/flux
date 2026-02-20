use std::env;
use std::time::Instant;

use crate::bytecode::symbol_table::SymbolTable;
use crate::runtime::{RuntimeContext, value::Value};
use crate::syntax::interner::Interner;

use super::VM;

pub enum TestOutcome {
    Pass,
    Fail(String),
}

pub struct TestResult {
    pub name: String,
    pub elapsed_ms: f64,
    pub outcome: TestOutcome,
}

/// Collects test function names and their global slot indices from the symbol table.
///
/// Discovers:
/// - Top-level functions whose name starts with `"test_"`
/// - Functions inside a `Tests` module (`"Tests.test_*"`)
///
/// Results are sorted by global index to preserve definition order.
pub fn collect_test_functions(
    symbol_table: &SymbolTable,
    interner: &Interner,
) -> Vec<(String, usize)> {
    let mut tests: Vec<(String, usize)> = symbol_table
        .global_definitions()
        .into_iter()
        .filter_map(|(sym, idx)| {
            let name = interner.resolve(sym);
            if name.starts_with("test_")
                || (name.starts_with("Tests.") && name["Tests.".len()..].starts_with("test_"))
            {
                Some((name.to_string(), idx))
            } else {
                None
            }
        })
        .collect();

    // Sort by global slot index to preserve source definition order.
    tests.sort_by_key(|(_, idx)| *idx);
    tests
}

/// Resolves `(name, global_index)` pairs into `(name, Value)` by looking
/// up each index in the provided globals slice. Values are cloned (cheap
/// for `Rc`-based closures).
fn extract_test_fns(globals: &[Value], tests: Vec<(String, usize)>) -> Vec<(String, Value)> {
    tests
        .into_iter()
        .map(|(name, idx)| (name, globals[idx].clone()))
        .collect()
}

/// Runs a resolved list of `(name, Value)` test functions via `invoke_value`
/// on any `RuntimeContext` (VM or JIT). Returns the per-test results.
pub fn run_test_fns(ctx: &mut dyn RuntimeContext, fns: Vec<(String, Value)>) -> Vec<TestResult> {
    let mut results = Vec::new();

    for (name, fn_value) in fns {
        let start = Instant::now();
        let outcome = match ctx.invoke_value(fn_value, vec![]) {
            Ok(_) => TestOutcome::Pass,
            Err(msg) => TestOutcome::Fail(msg),
        };
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        results.push(TestResult {
            name,
            elapsed_ms,
            outcome,
        });
    }

    results
}

/// VM convenience: extract test values from `vm.globals` then run them.
pub fn run_tests(vm: &mut VM, tests: Vec<(String, usize)>) -> Vec<TestResult> {
    let fns = extract_test_fns(&vm.globals, tests);
    run_test_fns(vm, fns)
}

/// JIT convenience: extract test values from `ctx.globals` then run them.
/// Only available when the `jit` feature is enabled.
#[cfg(feature = "jit")]
pub fn run_tests_jit(
    ctx: &mut crate::jit::context::JitContext,
    tests: Vec<(String, usize)>,
) -> Vec<TestResult> {
    let fns = extract_test_fns(&ctx.globals, tests);
    run_test_fns(ctx, fns)
}

/// Prints the test report and returns `true` if all tests passed.
pub fn print_test_report(file_name: &str, results: &[TestResult]) -> bool {
    println!("Running tests in {}\n", file_name);
    let use_color = colors_enabled();

    let mut passed = 0usize;
    let mut failed = 0usize;
    let grouped_output = results.iter().any(|r| r.name.starts_with("Tests."));
    let mut current_group: Option<&str> = None;

    for result in results {
        let (group, display_name) = if let Some(rest) = result.name.strip_prefix("Tests.") {
            ("Tests", rest)
        } else {
            ("top-level", result.name.as_str())
        };

        if grouped_output && current_group != Some(group) {
            if current_group.is_some() {
                println!();
            }
            println!("  [{}]", group);
            current_group = Some(group);
        }

        match &result.outcome {
            TestOutcome::Pass => {
                let shown_name = if grouped_output {
                    display_name
                } else {
                    result.name.as_str()
                };
                let pass = if use_color {
                    green("PASS")
                } else {
                    "PASS".to_string()
                };
                let timing = if use_color {
                    cyan_dim(&format!("({:.0}ms)", result.elapsed_ms))
                } else {
                    format!("({:.0}ms)", result.elapsed_ms)
                };
                println!("  {}  {:<34} {}", pass, shown_name, timing);
                passed += 1;
            }
            TestOutcome::Fail(msg) => {
                let fail = if use_color {
                    red("FAIL")
                } else {
                    "FAIL".to_string()
                };
                println!(
                    "  {}  {}",
                    fail,
                    if grouped_output {
                        display_name
                    } else {
                        result.name.as_str()
                    }
                );
                for line in msg.lines() {
                    println!("          {}", line);
                }
                failed += 1;
            }
        }
    }

    let total = passed + failed;
    println!("\n{} tests: {} passed, {} failed", total, passed, failed);

    if failed == 0 {
        if use_color {
            println!("\n{}", green("OK"));
        } else {
            println!("\nOK");
        }
        true
    } else {
        if use_color {
            println!("\n{}", red("FAILED"));
        } else {
            println!("\nFAILED");
        }
        false
    }
}

fn colors_enabled() -> bool {
    if env::var_os("NO_COLOR").is_some() {
        return false;
    }
    !matches!(env::var("TERM").ok().as_deref(), Some("dumb"))
}

fn green(s: &str) -> String {
    format!("\x1b[32m{}\x1b[0m", s)
}

fn red(s: &str) -> String {
    format!("\x1b[31m{}\x1b[0m", s)
}

fn cyan_dim(s: &str) -> String {
    format!("\x1b[36;2m{}\x1b[0m", s)
}
