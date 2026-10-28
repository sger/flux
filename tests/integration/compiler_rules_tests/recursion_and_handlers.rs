use super::*;

#[test]
fn forward_reference_nested_call() {
    // Function a calls b, b calls c, c is defined last
    compile_ok_in("test.flx", "fn a() { b(); } fn b() { c(); } fn c() { 42; }");
}

#[test]
fn mutual_recursion_two_functions() {
    // Functions f and g call each other
    compile_ok_in(
        "test.flx",
        "fn f(x) { if x > 0 { g(x - 1); } else { 0; } } fn g(x) { if x > 0 { f(x - 1); } else { 1; } }",
    );
}

#[test]
fn mutual_recursion_three_functions() {
    // Functions a, b, c form a circular dependency
    compile_ok_in(
        "test.flx",
        "fn a(x) { if x > 0 { b(x - 1); } else { 0; } } fn b(x) { if x > 0 { c(x - 1); } else { 1; } } fn c(x) { if x > 0 { a(x - 1); } else { 2; } }",
    );
}

#[test]
fn self_recursion_still_works() {
    // Ensure basic recursion still works
    compile_ok_in("test.flx", "fn factorial(n) { if n < 2 { 1; } else { n * factorial(n - 1); } }");
}

#[test]
fn hm_fixture_140_recursive_self_reference_refines_type() {
    let source = include_str!(
        "../../../examples/type_system/failing/140_recursive_self_reference_return_precision.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(rendered.contains("error[E300]"), "expected E300 for fixture 140, got:\n{}", rendered);
    assert!(
        rendered.contains("does not match its type annotation")
            && rendered.contains("String")
            && rendered.contains("Int"),
        "expected concrete downstream typed-let mismatch in fixture 140, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_141_recursive_self_reference_guard_no_regression() {
    let source = include_str!(
        "../../../examples/type_system/failing/141_recursive_self_reference_negative_guard.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E056]"),
        "expected independent follow-up E056 in fixture 141, got:\n{}",
        rendered
    );
}

// Base function tests removed — base functions are no longer registered in
// the symbol table. The Flux stdlib (`lib/Flow/*.flx`) replaces the Rust
// base function registry (Proposal 0120).

#[test]
fn import_non_base_except_is_accepted() {
    compile_ok_in("test.flx", "import Foo except [drop]\n1;");
}

#[test]
fn import_non_base_alias_except_is_accepted() {
    compile_ok_in("test.flx", "import Foo as F except [drop]\n1;");
}

#[test]
fn forward_reference_with_variables() {
    // Forward reference with let bindings in between
    compile_ok_in("test.flx", "fn f() { g(); } let x = 10; fn g() { x; }");
}

#[test]
fn duplicate_function_still_errors() {
    // Ensure duplicate function names still produce an error
    let code = compile_err("fn f() { 1; } fn f() { 2; }");
    assert_eq!(code, "E001");
}

#[test]
fn module_forward_reference() {
    // Function in module uses another function defined later in the same module
    compile_ok_in(
        "test.flx",
        "module Math { fn quadruple(x) { double(double(x)); } fn double(x) { x * 2; } }",
    );
}

#[test]
fn module_mutual_recursion() {
    // Functions within a module call each other
    compile_ok_in(
        "test.flx",
        "module Parity { fn isEven(n) { if n == 0 { true; } else { isOdd(n - 1); } } fn isOdd(n) { if n == 0 { false; } else { isEven(n - 1); } } }",
    );
}

/// Proposal 0161 B1: `alias Name = <E1 | E2 | ...>` declarations declare a
/// compile-time effect-row alias. Any `with Alias` clause is expanded to the
/// named row before type inference runs.
#[test]
fn effect_row_alias_single_expands_to_declared_effect() {
    // `Stdout` expands to `<IO>`. A caller that declares `with IO` can call
    // a callee declared `with Stdout` — they must type-check as the same row.
    compile_ok_in(
        "alias_single.flx",
        r#"
alias Stdout = <IO>

fn shout(msg: String) with Stdout {
    println(msg)
}

fn main() with IO {
    shout("hi")
}
"#,
    );
}

#[test]
fn effect_row_alias_multi_expands_to_decomposed_row() {
    // Caller declares `with IO, Time`; callee declares `with Observable`
    // which expands to `<IO | Time>`. Both rows must match.
    compile_ok_in(
        "alias_multi.flx",
        r#"
alias Observable = <IO | Time>

fn log_event(msg: String) with Observable {
    let t = clock_now()
    println(msg)
}

fn main() with IO, Time {
    log_event("start")
}
"#,
    );
}

#[test]
fn effect_row_alias_missing_component_reports_e400() {
    // Callee declares `with Observable` (= `<IO | Time>`) but caller only
    // declares `with IO`. The alias must be expanded before the E400 check
    // runs, so Time is reported missing — not "effect Observable".
    let code = compile_err(
        r#"
alias Observable = <IO | Time>

fn log_event(msg: String) with Observable {
    println(msg)
}

fn caller() with IO {
    log_event("x")
}

fn main() {
    caller()
}
"#,
    );
    assert_eq!(code, "E400");
}

/// Proposal 0161 B3: fine-grained `with Console` annotations are accepted
/// as first-class. The builtin `IO = <Console | FileSystem | Stdin>` seed
/// means a caller with `with Console` can invoke any `println`-shaped
/// function without widening to `IO`.
#[test]
fn fine_grained_console_annotation_accepted() {
    compile_ok_in(
        "fine_console.flx",
        r#"
fn shout(msg: String) with Console {
    println(msg)
}

fn main() with Console {
    shout("hi")
}
"#,
    );
}

/// Proposal 0161 B3: a function annotated `with IO` still accepts all
/// three decomposed primops because the alias seeding expands `IO` into
/// `<Console | FileSystem | Stdin>` before the effect check runs.
#[test]
fn with_io_still_covers_all_decomposed_primops() {
    compile_ok_in(
        "with_io_decomposed.flx",
        r#"
fn do_all() with IO {
    println("ping")
}

fn main() with IO {
    do_all()
}
"#,
    );
}

#[test]
fn sealing_allows_matching_fine_grained_effect() {
    compile_ok_in(
        "sealing_console_ok.flx",
        r#"
fn main() with Console {
    println("ping") sealing { Console }
}
"#,
    );
}

#[test]
fn sealing_denies_effect_outside_allowed_row() {
    let code = compile_err(
        r#"
fn main() with IO {
    read_file("missing.txt") sealing { Console }
}
"#,
    );
    assert_eq!(code, "E427");
}

#[test]
fn sealing_expands_builtin_io_alias() {
    compile_ok_in(
        "sealing_io_alias.flx",
        r#"
fn main() with IO {
    println("ping") sealing { IO }
}
"#,
    );
}

#[test]
fn sealing_nested_restrictions_compose() {
    compile_ok_in(
        "sealing_nested.flx",
        r#"
fn main() with Console {
    (println("ping") sealing { Console }) sealing { Console }
}
"#,
    );
}

#[test]
fn sealing_polymorphic_callback_subset() {
    compile_ok_in(
        "sealing_callback.flx",
        r#"
fn run(test_fn: (() -> Unit with Console)) with Console {
    test_fn() sealing { Console }
}

fn main() with Console {
    fn callback() with Console {
        println("ping")
    }
    run(callback)
}
"#,
    );
}

#[test]
fn sealing_ambient_subtraction_allows_remaining_effects() {
    compile_ok_in(
        "sealing_ambient_subtract_ok.flx",
        r#"
fn main() with IO {
    println("ping") sealing (ambient - FileSystem)
}
"#,
    );
}

#[test]
fn sealing_ambient_subtraction_denies_removed_effect() {
    let code = compile_err(
        r#"
fn main() with IO {
    read_file("missing.txt") sealing (ambient - FileSystem)
}
"#,
    );
    assert_eq!(code, "E427");
}

#[test]
fn parameterized_handler_discharges_effect_before_sealing() {
    compile_ok_in(
        "parameterized_handler_sealing.flx",
        r#"
effect State {
    get: () -> Int
}

fn program() -> Int with State {
    perform State.get()
}

fn main() {
    let _ = (program() handle State(7) {
        get(resume, state) -> resume(state, state)
    }) sealing { Console }
}
"#,
    );
}

#[test]
fn parameterized_handler_initializer_keeps_own_effect_requirement() {
    let code = compile_err_strict(
        r#"
effect State {
    get: () -> Int
}

fn init() -> Int with Console {
    println("init")
    0
}

fn program() -> Int with State {
    perform State.get()
}

fn run() {
    let _ = program() handle State(init()) {
        get(resume, state) -> resume(state, state)
    }
}
"#,
    );
    assert_eq!(code, "E415");
}

#[test]
fn parameterized_handler_rejects_builtin_effect_alias_target() {
    let message = compile_err_message(
        r#"
fn main() with IO {
    let _ = println("hidden") handle IO(0) {
        println(resume, _value, state) -> resume((), state)
    }
}
"#,
    );
    assert!(
        message.contains("Effect `IO`") || message.contains("Handler for `IO`"),
        "expected a clear diagnostic for alias-row handler target, got: {message}"
    );
}
