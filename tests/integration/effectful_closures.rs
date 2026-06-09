//! Effectful closures end-to-end.
//!
//! Before this fix the compiler's effect re-check was stricter than the type
//! checker: an un-annotated closure that performed an effect (e.g. a fiber
//! `Channel.send`) was rejected with E400 even though inference accepted it, so
//! only named top-level functions could be effectful. These tests pin the fixed
//! behaviour:
//!   - Part A: an un-annotated function literal inherits its enclosing function's
//!     ambient effect row (compiler now agrees with inference).
//!   - Part B: a literal may carry an explicit `fn(...) -> T with E { ... }`.
//!   - Soundness: a closure may NOT perform an effect its enclosing function lacks.

#[path = "../support/flux_runner.rs"]
mod flux_runner;

#[test]
fn unannotated_effectful_closure_passed_to_both_compiles_and_runs() {
    // The closures perform Async effects (yield_now, Channel.send) with no `with`
    // clause; they inherit `body`'s ambient `Async`. Previously E400.
    let source = r#"
    import Flow.Async exposing (..)
    import Flow.Channel as Channel

    fn body() -> String with Async {
        let ch = Channel.make(8)
        both(
            fn() { yield_now(); Channel.send(ch, "A") },
            fn() { yield_now(); Channel.send(ch, "B") },
        )
        match (Channel.recv(ch), Channel.recv(ch)) {
            (Some(x), Some(y)) -> x + y,
            _ -> "closed"
        }
    }

    fn main() with IO {
        print(run_async(body))
    }
    "#;
    let (stdout, stderr, success) = flux_runner::run_flux(source, "closure_both");
    assert!(
        success,
        "un-annotated effectful closure must compile and run:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // Both messages were sent and received (order is scheduler-defined).
    let out = stdout.trim();
    assert!(
        out == "\"AB\"" || out == "\"BA\"",
        "unexpected output: {out:?}"
    );
}

#[test]
fn let_bound_effectful_closure_compiles_and_runs() {
    let source = r#"
import Flow.Async exposing (..)
import Flow.Channel as Channel

fn body() -> String with Async {
    let ch = Channel.make(8)
    let producer = fn() { Channel.send(ch, "hi") }
    both(producer, fn() { yield_now() })
    match Channel.recv(ch) { Some(v) -> v, _ -> "none" }
}

fn main() with IO {
    print(run_async(body))
}
    "#;
    let (stdout, stderr, success) = flux_runner::run_flux(source, "closure_let");
    assert!(
        success,
        "let-bound effectful closure must compile and run:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "\"hi\"");
}

#[test]
fn explicit_effect_annotation_on_closure_compiles_and_runs() {
    // explicit `fn() -> T with E { ... }` literal syntax.
    let source = r#"
import Flow.Async exposing (..)
import Flow.Channel as Channel

fn body() -> String with Async {
    let ch = Channel.make(8)
    both(
        fn() -> Unit with Async { Channel.send(ch, "x") },
        fn() -> Unit with Async { yield_now() }
    )
    match Channel.recv(ch) { Some(v) -> v, _ -> "none" }
}

fn main() with IO {
    print(run_async(body))
}
"#;

    let (stdout, stderr, success) = flux_runner::run_flux(source, "closure_explicit");
    assert!(
        success,
        "explicitly-annotated effectful closure must compile and run:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "\"x\"");
}

#[test]
fn closure_effect_beyond_enclosing_function_is_rejected() {
    // Soundness: the closure performs `Console` (print) but the enclosing
    // `pure_ctx` declares no effects — this must still fail to compile.
    let source = r#"
fn apply(f: () -> Int) -> Int { f() }

fn pure_ctx() -> Int {
    apply(fn() { print("leak"); 1 })
}

fn main() with IO {
    print(to_string(pure_ctx()))
}
"#;
    let (stdout, stderr, success) = flux_runner::run_flux(source, "closure_unsound");
    assert!(
        !success,
        "closure performing an effect beyond its enclosing function must be rejected:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    assert!(
        stderr.contains("E400") || stderr.contains("Missing Ambient Effect"),
        "expected E400 missing-effect error, got stderr:\n{stderr}"
    );
}
