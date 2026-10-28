use super::*;

#[test]
fn repl_reports_a_typed_hole_with_type_and_fits() {
    // Typing `_` reports a typed hole (GHC-style): its expected type plus the
    // in-scope bindings that fit — instead of evaluating.
    let output = run_flux_repl("map([1, 2, 3], _)\n:quit\n");
    let combined = combined_output(&output);
    assert!(
        combined.contains("E469") && combined.contains("found hole"),
        "expected a typed-hole diagnostic, output:\n{combined}",
    );
    assert!(
        combined.contains("(Int) ->") && combined.contains("relevant bindings"),
        "expected the hole's function type and fitting bindings, output:\n{combined}",
    );
}

#[test]
fn repl_continues_after_a_typed_hole() {
    // A hole is reported (not a crash) and the session keeps going.
    let output = run_flux_repl("1 + _\n2 + 2\n:quit\n");
    assert!(
        combined_output(&output).contains("found hole"),
        "expected a hole report, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        repl_results(&output).iter().any(|line| line == "4"),
        "the session must keep evaluating after a hole, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn repl_runs_effects_once_without_replaying() {
    // Phase 2 (proposal 0176): each line runs only its own delta on the live VM,
    // so an effectful expression's output appears exactly once. The Phase 1
    // engine re-ran the whole session every line and would have replayed it.
    let output = run_flux_repl("println(\"effect\")\n1 + 1\n:quit\n");
    let effect_lines =
        repl_results(&output).into_iter().filter(|line| line.contains("effect")).count();
    assert_eq!(
        effect_lines,
        1,
        "the effect must run exactly once, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        repl_results(&output).iter().any(|line| line == "2"),
        "the later pure line still evaluates, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn repl_runs_effectful_declaration_once() {
    // A top-level declaration whose initializer is effectful (e.g.
    // `let _ = println("effect")`) used to fail with E413 / E414. The REPL now
    // re-runs it inside a synthesized `main`, so the effect fires exactly once
    // and a later pure line still evaluates (proposal 0176).
    let output = run_flux_repl("let _ = println(\"effect\")\n1 + 1\n:quit\n");
    let effect_lines =
        repl_results(&output).into_iter().filter(|line| line.contains("effect")).count();
    assert_eq!(
        effect_lines,
        1,
        "the declaration's effect must run exactly once, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        repl_results(&output).iter().any(|line| line == "2"),
        "the later pure line still evaluates, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn repl_rebinds_an_existing_name() {
    // Phase 2 keeps one persistent compiler for the session, so redefining a
    // name shadows the old binding (on a fresh global slot) instead of erroring
    // as a duplicate — `x` reads `1`, then `99` after the rebind.
    let output = run_flux_repl("let x = 1\nx\nlet x = 99\nx\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["1", "99"],
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_evaluates_top_level_control_flow() {
    // Top-level `if` / `match` emit jumps with absolute targets into the session
    // buffer; the engine runs the full buffer from the line's offset so those
    // targets resolve (a delta run at offset 0 would mis-jump). A non-constant
    // condition keeps the optimizer from folding the branch away.
    let output = run_flux_repl(
        "let c = 1\nif c > 0 { 111 } else { 222 }\nmatch c { 1 -> \"one\", _ -> \"other\" }\n:quit\n",
    );
    assert_eq!(
        repl_results(&output),
        ["111", "\"one\""],
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_type_works_after_rebinding() {
    // Rebinding must not leave a duplicate definition in the `:type` record:
    // its fresh re-inference compile would otherwise reject the second `let x`.
    let output = run_flux_repl("let x = 5\nlet x = 99\n:type x + 1\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["x + 1 : Int"],
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_continues_multiline_forms() {
    // A form left open by a `{` continues onto further physical lines until the
    // delimiters balance, then compiles as one declaration. This exercises the
    // multi-line continuation shared by the interactive and piped input paths.
    let output = run_flux_repl("fn double(n: Int) -> Int {\n  n * 2\n}\ndouble(21)\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["42"],
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_load_brings_file_definitions_into_scope() {
    // `:load` compiles the whole file in one delta, so its definitions are usable
    // on later lines (here `triple` applied to the loaded `base`).
    let (_files, file) =
        repl_temp_flx("load", "fn triple(n: Int) -> Int { n * 3 }\nlet base = 7\n");
    let output = run_flux_repl(&format!(":load \"{}\"\ntriple(base)\n:quit\n", file.display()));
    assert_eq!(
        repl_results(&output),
        ["21"],
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_reload_reapplies_the_loaded_file() {
    // After `:load`, `:reload` re-bootstraps and re-applies the same file, so its
    // definitions remain callable.
    let (_files, file) = repl_temp_flx("reload", "fn answer() -> Int { 42 }\n");
    let output =
        run_flux_repl(&format!(":load \"{}\"\n:reload\nanswer()\n:quit\n", file.display()));
    assert_eq!(
        repl_results(&output),
        ["42"],
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_reload_without_load_reports_nothing() {
    let output = run_flux_repl(":reload\n:quit\n");
    assert!(
        combined_output(&output).contains("Nothing to reload"),
        "expected a 'nothing to reload' message, output:\n{}",
        combined_output(&output),
    );
}

#[test]
fn repl_load_missing_file_keeps_session() {
    // A failed `:load` reports the error and leaves the session usable: the
    // following `1 + 1` still evaluates.
    let output = run_flux_repl(":load definitely_missing_repl_file.flx\n1 + 1\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["2"],
        "stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        combined_output(&output).contains("cannot read"),
        "expected a 'cannot read' error, output:\n{}",
        combined_output(&output),
    );
}

#[test]
fn repl_list_shows_session_declarations() {
    // `:list` echoes the committed session declarations (to stderr). A pure
    // `let` and a `fn` should both appear verbatim.
    let output = run_flux_repl("let a = 1\nfn f(n) { n + 1 }\n:list\n:quit\n");
    let combined = combined_output(&output);
    assert!(combined.contains("let a = 1"), "`:list` should show the `let`, output:\n{combined}",);
    assert!(
        combined.contains("fn f(n) { n + 1 }"),
        "`:list` should show the `fn`, output:\n{combined}",
    );
}

#[test]
fn repl_help_lists_commands() {
    // `:help` and its `:?` alias both print the command list (to stderr).
    for command in [":help", ":?"] {
        let output = run_flux_repl(&format!("{command}\n:quit\n"));
        let combined = combined_output(&output);
        for expected in [":type", ":info", ":browse", ":set", ":load", ":reset", ":list", ":quit"] {
            assert!(
                combined.contains(expected),
                "`{command}` should mention `{expected}`, output:\n{combined}",
            );
        }
    }
}

#[test]
fn repl_reset_after_load_clears_loaded_definitions() {
    // `:reset` forgets everything brought in by `:load`: `triple` resolves before
    // the reset (one `21`) and is gone after it (the second call errors, so no
    // second `21`).
    let (_files, file) = repl_temp_flx("reset_after_load", "fn triple(n: Int) -> Int { n * 3 }\n");
    let output = run_flux_repl(&format!(
        ":load \"{}\"\ntriple(7)\n:reset\ntriple(7)\n:quit\n",
        file.display()
    ));
    let twenty_ones = repl_results(&output).into_iter().filter(|line| line == "21").count();
    assert_eq!(
        twenty_ones,
        1,
        "`triple` should resolve before `:reset` and be gone after, output:\n{}",
        combined_output(&output),
    );
    assert!(
        combined_output(&output).contains("Session reset."),
        "`:reset` should confirm, output:\n{}",
        combined_output(&output),
    );
}

#[test]
fn repl_effectful_expression_does_not_update_it() {
    // A documented v1 limitation: an effectful expression runs but its result is
    // not captured by `it`. So after `21 * 2` (it = 42), running `println("hi")`
    // leaves `it` at 42, which the final line prints.
    let output = run_flux_repl("21 * 2\nprintln(\"hi\")\nit\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["42", "\"hi\"", "42"],
        "effectful expression must not rebind `it`, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn repl_uses_effect_declared_on_an_earlier_line() {
    // Proposal 0176: a user `effect` declared on one line is usable on later
    // lines (it persists in the session's effect registry instead of resetting
    // to the prelude-only set each compile, which used to report E407).
    let output = run_flux_repl(concat!(
        "effect Audit { log: String -> Int }\n",
        "fn audited() -> Int with Audit { perform Audit.log(\"started\") + 1 }\n",
        "audited() handle Audit { log(resume, message) -> resume(len(message)) }\n",
        ":quit\n",
    ));
    assert_eq!(
        repl_results(&output),
        ["8"],
        "cross-line effect should resolve and run, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_uses_class_and_instance_declared_on_earlier_lines() {
    // Proposal 0176: a user `class` and its `instance`s declared on earlier
    // lines stay in scope, so a later line can dispatch the method across them
    // (previously the class env was rebuilt per compile and the method was
    // reported undefined, E004).
    let output = run_flux_repl(concat!(
        "class Sizeable<a> { fn size(x: a) -> Int }\n",
        "instance Sizeable<Int> { fn size(x) { 1 } }\n",
        "instance Sizeable<String> { fn size(x) { len(x) } }\n",
        "size(42) + size(\"hello\")\n",
        ":quit\n",
    ));
    assert_eq!(
        repl_results(&output),
        ["6"],
        "cross-line class method should dispatch, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_captures_effectful_expression_result_into_it() {
    // Proposal 0176: an effectful expression runs inside `main` (the only context
    // with the root IO handler), and its primitive result is still captured into
    // `it`. The `do` block prints once and returns 99; the effect must run exactly
    // once (one "side" line), 99 is echoed as the captured result, and `it` then
    // resolves to 99 so `it + 1` is 100.
    let output = run_flux_repl("do {\n  print(\"side\")\n  99\n}\nit + 1\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["\"side\"", "99", "100"],
        "effectful result should run once and be captured into `it`, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_effectful_statement_result_is_not_captured() {
    // A statement-effect that returns nothing (`print`) keeps its output but is
    // not echoed or captured: `it` still refers to the earlier `7`.
    let output = run_flux_repl("let x = 7\nx\nprint(\"hi\")\nit\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["7", "\"hi\"", "7"],
        "print should not overwrite `it`, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_captures_compound_effectful_result_into_it() {
    // Proposal 0176: an effectful expression returning a *compound* value (a list,
    // tuple, or user ADT) is now captured into `it`, not just a primitive — the
    // result is re-bound as a Flux literal (`[10, 20]`, `(1, 2)`, `Box(7)`) that
    // re-parses to the same value without re-running the effect. Each `do` block
    // prints once; the captured value is echoed; then `it` is used as a real value
    // (`len`, tuple access, `match`) on the next line. The ADT's constructor name
    // stays in scope via the accumulated `data` decl.
    // The list/tuple are `let`-bound inside the `do` block so the literal doesn't
    // sit at the start of a line after another expression — a leading `[`/`(` there
    // is parsed as an index/call on the previous expression (an unrelated parser
    // quirk). The ADT tail starts with an identifier, so it needs no such dance.
    let output = run_flux_repl(concat!(
        "do {\n  print(\"a\")\n  let xs = [10, 20]\n  xs\n}\n",
        "len(it)\n",
        "do {\n  print(\"b\")\n  let t = (1, 2)\n  t\n}\n",
        "match it {\n  (a, b) -> a + b\n}\n",
        "type Box = Box(Int)\n",
        "do {\n  print(\"c\")\n  Box(7)\n}\n",
        "match it {\n  Box(n) -> n\n}\n",
        ":quit\n",
    ));
    assert_eq!(
        repl_results(&output),
        ["\"a\"", "[10, 20]", "2", "\"b\"", "(1, 2)", "3", "\"c\"", "Box(7)", "7"],
        "compound effectful results should be captured into `it`, stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_persists_named_effectful_binding() {
    // Proposal 0176: `let x = <effectful>` now persists. The initializer is
    // rejected at top level (E413), so just the initializer runs inside `main`
    // (effect fires once), its value is captured, and `x` is re-bound to that value
    // purely — so a later line sees `x`. The `do` block prints once ("e"); `let`
    // itself echoes nothing; then `x + 1` is 43. A compound captured value persists
    // too: `ys` is `[1, 2, 3]`, so `len(ys)` is 3.
    let output = run_flux_repl(concat!(
        "let x = do {\n  print(\"e\")\n  42\n}\n",
        "x + 1\n",
        "let ys = do {\n  print(\"f\")\n  let zs = [1, 2, 3]\n  zs\n}\n",
        "len(ys)\n",
        ":quit\n",
    ));
    assert_eq!(
        repl_results(&output),
        ["\"e\"", "43", "\"f\"", "3"],
        "named effectful binding should persist its captured value, stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_self_referential_rebind_reads_previous_value() {
    // Proposal 0176: `let x = x + 1` rebinding an existing session binding reads
    // the *previous* value (11), then `let x = x * 2` reads that (22). Previously
    // the initializer saw a freshly-defined, uninitialized slot and the line
    // failed with a None-arithmetic runtime error. `let` declarations print
    // nothing, so only the bare `x` lines produce output.
    let output = run_flux_repl("let x = 10\nlet x = x + 1\nx\nlet x = x * 2\nx\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["11", "22"],
        "self-referential rebind should read the previous value, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_self_referential_rebind_allows_type_change() {
    // The capture-then-rebind path is value-based, so a rebind may change the
    // binding's type: `xs : [Int]` becomes `xs : Int` via `len(xs)`.
    let output = run_flux_repl("let xs = [1, 2, 3]\nlet xs = len(xs)\nxs + 1\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["4"],
        "self-referential rebind should allow a type change, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_uses_named_field_data_declared_on_an_earlier_line() {
    // Proposal 0176: a `data` type with named fields declared on one line is
    // fully usable on later lines — construction, dot access, and functional
    // spread-update. The session accumulates the `data` declaration so later
    // lines' inference + named-field desugar see its field metadata; without
    // it, construction reported E082/E430 and spread reported E464. (Unblocked
    // by the top-level-frame codegen fix, which makes the spread's desugared
    // `match` compile correctly at REPL top level.)
    let output = run_flux_repl(concat!(
        "data Person { Person { name: String, age: Int } }\n",
        "let alice = Person { name: \"Alice\", age: 30 }\n",
        "alice.age\n",
        "let bob = { ...alice, name: \"Bob\", age: alice.age + 1 }\n",
        "bob.name\n",
        "bob.age\n",
        ":quit\n",
    ));
    assert_eq!(
        repl_results(&output),
        ["30", "\"Bob\"", "31"],
        "cross-line named-field construct / access / spread should work, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_rejects_unknown_field_on_cross_line_named_data() {
    // The accumulated `data` metadata is scoped, not a blanket pass: an unknown
    // field on an earlier-line record still errors (E463) rather than silently
    // resolving — the line is rolled back and produces no value.
    let output = run_flux_repl(concat!(
        "data Point { Point { x: Int, y: Int } }\n",
        "let p = Point { x: 1, y: 2 }\n",
        "p.z\n",
        ":quit\n",
    ));
    assert!(
        repl_results(&output).is_empty(),
        "unknown field must not yield a value, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("E463"),
        "expected E463 for unknown field, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn run_compiles_top_level_match_and_destructure() {
    // The cached/parallel compile path (module linker) used to fail with
    // "missing global mapping" when a top-level `match` or tuple `let (a, b)`
    // allocated a transient slot in the global namespace. Run a file end to end
    // through `flux <file>` (which exercises that path) and check it executes.
    // `run_flux` gives the run its own cache dir, so this test exercises the
    // cached module-linker path without sharing a cache with concurrent
    // tests (which would race the prelude artifacts: "interface fingerprint
    // changed").
    let scratch = Scratch::new("toplevel-frame");
    let file = scratch.write(
        "toplevel_frame.flx",
        "let x = match Some(7) { Some(n) -> n, _ -> 0 }\n\
         let (a, b) = (10, 20)\n\
         fn main() with IO { println(x + a + b) }\n",
    );
    let output = run_flux(&[file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.lines().any(|line| line.trim() == "37"),
        "expected 37 from top-level match + destructure, stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}
