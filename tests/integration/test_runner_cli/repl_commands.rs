use super::*;

#[test]
fn repl_evaluates_and_remembers_bindings() {
    // A later line sees an earlier `let`; declarations themselves print nothing.
    let output = run_flux_repl("1 + 2\nlet x = 10\nx + 1\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["3", "11"],
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_binds_it_to_the_last_result() {
    let output = run_flux_repl("21 * 2\nit + 1\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["42", "43"],
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_supports_core_language_surface() {
    // Coverage guard: the persistent REPL must accept the full Flux syntax surface,
    // including cross-line use of declarations. Each entry below is one REPL line;
    // declarations (`let` / `fn` / `type` / `data` / `effect`) echo nothing, every
    // other line echoes its result. If a construct ever stops parsing/compiling in
    // the REPL, the corresponding result line drops out and this fails. (Exercised
    // here: arithmetic, float, string concat + interpolation, booleans, lambdas and
    // immediate application, lists / arrays / maps, list comprehensions with
    // filters, `if` / `match` with Option / cons-pattern / guard, tuples, functions
    // with recursion and generics, positional ADTs, named-field `data` with
    // construction / field access / spread-update, the pipe operator, and a user
    // `effect` with `perform` / `handle`.)
    let output = run_flux_repl(concat!(
        "1 + 2 * 3\n",
        "3.0 * 2.0\n",
        "\"a\" + \"b\"\n",
        "3 > 2 && 1 < 2\n",
        "(\\x -> x * 2)(5)\n",
        "[1, 2, 3]\n",
        "len([1, 2, 3])\n",
        "map([1, 2, 3], \\x -> x * 2)\n",
        "[|10, 20, 30|]\n",
        "{\"a\": 1}\n",
        "if 3 > 2 { \"yes\" } else { \"no\" }\n",
        "match Some(5) { Some(n) -> n, None -> 0 }\n",
        "(1, \"a\", true)\n",
        "[x * 2 | x <- [1, 2, 3]]\n",
        "[x | x <- [1, 2, 3, 4], x % 2 == 0]\n",
        "let name = \"world\"\n",
        "\"hi #{name}\"\n",
        "fn sq(n: Int) -> Int { n * n }\n",
        "sq(4)\n",
        "fn fac(n: Int) -> Int { if n <= 1 { 1 } else { n * fac(n - 1) } }\n",
        "fac(5)\n",
        "fn id<a>(x: a) -> a { x }\n",
        "id(7)\n",
        "type Color = Red | Green\n",
        "Red\n",
        "data Point { Point { x: Int, y: Int } }\n",
        "let p = Point { x: 3, y: 4 }\n",
        "p.x + p.y\n",
        "{ ...p, y: 100 }\n",
        "fn dbl(x) { x * 2 }\n",
        "5 |> dbl\n",
        "match [1, 2, 3] { [h | t] -> h, [] -> 0 }\n",
        "match 5 { n if n > 3 -> \"big\", _ -> \"small\" }\n",
        "effect Log { emit: String -> Int }\n",
        "fn go() -> Int with Log { perform Log.emit(\"hi\") + 1 }\n",
        "go() handle Log { emit(resume, msg) -> resume(len(msg)) }\n",
        ":quit\n",
    ));
    assert_eq!(
        repl_results(&output),
        [
            "7",
            "6",
            "\"ab\"",
            "true",
            "10",
            "[1, 2, 3]",
            "3",
            "[2, 4, 6]",
            "[|10, 20, 30|]",
            "{\"a\": 1}",
            "\"yes\"",
            "5",
            "(1, \"a\", true)",
            "[2, 4, 6]",
            "[2, 4]",
            "\"hi world\"",
            "16",
            "120",
            "7",
            "Red",
            "7",
            "Point(3, 100)",
            "10",
            "1",
            "\"big\"",
            "3",
        ],
        "REPL must accept the full language surface, stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_rolls_back_a_failed_line() {
    // `nope` is undefined: that line errors and is discarded, so the session
    // stays intact and `x` is still usable on the next line.
    let output = run_flux_repl("let x = 5\nnope + 1\nx + 1\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["6"],
        "stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        !combined_output(&output).contains("panicked"),
        "a bad line must report a diagnostic, not panic, output:\n{}",
        combined_output(&output),
    );
}

#[test]
fn repl_reset_clears_the_session() {
    // After `:reset`, the earlier `x` is gone, so `x + 1` errors and prints
    // nothing to stdout.
    let output = run_flux_repl("let x = 5\n:reset\nx + 1\n:quit\n");
    assert!(
        repl_results(&output).is_empty(),
        "stdout should be empty after reset, got:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        combined_output(&output).contains('x'),
        "expected an undefined-name error mentioning `x`, output:\n{}",
        combined_output(&output),
    );
}

#[test]
fn repl_type_reports_inferred_types() {
    // `:type` prints `<expr> : <type>`, sees session bindings, and resolves `it`
    // to the last result — interleaved with ordinary evaluation.
    let output = run_flux_repl("let xs = [1, 2, 3]\n:type xs\n21 * 2\n:type it\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["xs : List<Int>", "42", "it : Int"],
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_type_does_not_evaluate_the_expression() {
    // `:type` of a printing expression reports its type without running it: the
    // type line appears, but the side effect never does.
    let output = run_flux_repl(":type println(\"side effect\")\n:quit\n");
    let results = repl_results(&output);
    assert_eq!(
        results,
        ["println(\"side effect\") : Unit"],
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        !results.iter().any(|line| line == "side effect"),
        ":type must not execute the expression, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn repl_type_reports_errors_without_evaluating() {
    // A `:type` on an undefined name prints no type line and is reported (not a
    // panic); the session stays intact so a later expression still evaluates.
    let output = run_flux_repl(":type nope\n1 + 1\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["2"],
        "stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        !combined_output(&output).contains("panicked"),
        "a bad `:type` must report a diagnostic, not panic, output:\n{}",
        combined_output(&output),
    );
}

#[test]
fn repl_info_describes_a_builtin_type() {
    // `:info` on a built-in type (a `TypeConstructor`, not a registry ADT) lists
    // its constructors, each with the signature inferred via eta-expansion.
    let output = run_flux_repl(":info Option\n:quit\n");
    assert_repl_line(&output, "type Option");
    assert_repl_line(&output, "None : Option<_>");
    assert_repl_line(&output, "Some : (_) -> Option<_>");
}

#[test]
fn repl_info_describes_a_user_adt_with_field_types() {
    // A user `type` is found in the ADT registry; each constructor's signature
    // (including positional field types) comes from saturated inference.
    let output = run_flux_repl("type Color = Red | Green(Int)\n:info Color\n:quit\n");
    assert_repl_line(&output, "type Color");
    assert_repl_line(&output, "Red   : Color");
    assert_repl_line(&output, "Green : (Int) -> Color");
}

#[test]
fn repl_info_describes_a_constructor_and_its_adt() {
    // `:info` on a constructor shows its own signature, the owning ADT, then the
    // ADT's full block (GHCi shows the defining type for a constructor).
    let output = run_flux_repl("type Color = Red | Green(Int)\n:info Green\n:quit\n");
    assert_repl_line(&output, "Green : (Int) -> Color");
    assert_repl_line(&output, "-- constructor of Color");
    assert_repl_line(&output, "type Color");
}

#[test]
fn repl_info_describes_a_seeded_effect() {
    // A built-in effect lives in the effect-op registry after bootstrap, so
    // `:info` lists its operations.
    let output = run_flux_repl(":info Console\n:quit\n");
    let results = repl_results(&output);
    assert_repl_line(&output, "effect Console");
    assert!(
        results.iter().any(|line| line.starts_with("print")),
        "expected a `print*` operation line, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn repl_info_describes_a_user_effect() {
    // A user `effect` declared earlier persists in the session registry, so
    // `:info` reports its operation and signature.
    let output = run_flux_repl("effect Log { emit: String -> Int }\n:info Log\n:quit\n");
    assert_repl_line(&output, "effect Log");
    assert_repl_line(&output, "emit : (String) -> Int");
}

#[test]
fn repl_info_reports_a_session_value_and_origin() {
    // For a value, `:info` shows its type and where it's defined.
    let output = run_flux_repl("let x = 5\n:info x\n:quit\n");
    assert_repl_line(&output, "x : Int");
    assert_repl_line(&output, "-- Defined in the current session");
}

#[test]
fn repl_info_reports_a_library_member_origin() {
    // An auto-exposed library member resolves to its defining module.
    let output = run_flux_repl(":info map\n:quit\n");
    let results = repl_results(&output);
    assert!(
        results.iter().any(|line| line.starts_with("map :")),
        "expected a `map : ...` type line, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert_repl_line(&output, "-- Defined in module Flow.List");
}

#[test]
fn repl_info_alias_i_works() {
    // `:i` is the short alias for `:info`.
    let output = run_flux_repl(":i Option\n:quit\n");
    assert_repl_line(&output, "type Option");
}

#[test]
fn repl_info_unknown_name_reports_without_panicking() {
    // An unknown name prints no info block (the inference diagnostic goes to
    // stderr) and the session keeps going — like `:type` on an unknown name.
    let output = run_flux_repl(":info totallyunknownname\n1 + 1\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["2"],
        "stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        !combined_output(&output).contains("panicked"),
        "a bad `:info` must report a diagnostic, not panic, output:\n{}",
        combined_output(&output),
    );
}

#[test]
fn repl_browse_lists_session_bindings_with_types() {
    // `:browse` pairs the user's own bindings with their inferred types under a
    // `Session` group — unlike `:list`, which only echoes declaration sources.
    let output = run_flux_repl(concat!(
        "let x = 5\n",
        "fn greet(name: String) -> String { name }\n",
        ":browse\n",
        ":quit\n",
    ));
    assert_repl_line(&output, "Session:");
    assert_repl_line(&output, "x     : Int");
    assert_repl_line(&output, "greet : (String) -> String");
}

#[test]
fn repl_browse_lists_prelude_names_with_types() {
    // The prelude group lists auto-exposed library members with their schemes,
    // sourced in bulk from the persistent compiler (no per-name re-inference).
    let output = run_flux_repl(":browse\n:quit\n");
    let results = repl_results(&output);
    assert_repl_line(&output, "Prelude:");
    assert!(
        results.iter().any(|line| line.starts_with("map ")),
        "expected a `map` prelude entry, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn repl_browse_filters_by_name_prefix() {
    // `:browse <prefix>` keeps only matching names, across both groups.
    let output = run_flux_repl("let xray = 1\n:browse x\n:quit\n");
    let results = repl_results(&output);
    assert_repl_line(&output, "xray : Int");
    assert!(
        !results.iter().any(|line| line.starts_with("map ")),
        "prefix `x` must exclude `map`, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn repl_browse_empty_session_shows_a_hint() {
    // With no user bindings yet, the Session group shows a hint rather than names.
    let output = run_flux_repl(":browse\n:quit\n");
    let results = repl_results(&output);
    assert!(
        results.iter().any(|line| line.contains("no bindings yet")),
        "empty session should show a hint, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn repl_set_plus_t_prints_type_after_evaluation() {
    // `:set +t` echoes the expression's type (as `it : <type>`) after its value.
    let output = run_flux_repl(":set +t\n21 * 2\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["42", "it : Int"],
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_set_plus_t_does_not_type_declarations() {
    // `+t` applies only to expression results — a declaration prints no type.
    let output = run_flux_repl(":set +t\nlet a = 1\na + 1\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["2", "it : Int"],
        "only the expression should be typed, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn repl_set_plus_s_reports_timing() {
    // `:set +s` prints an elapsed-time line (to stderr, off the value stream).
    let output = run_flux_repl(":set +s\n1 + 1\n:quit\n");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("secs"),
        "expected a timing line on stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    // The value stream stays clean — just the result.
    assert_eq!(repl_results(&output), ["2"]);
}

#[test]
fn repl_unset_disables_an_option() {
    // `:unset +t` turns the type echo back off.
    let output = run_flux_repl(":set +t\n:unset +t\n5 + 5\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["10"],
        "no type line after :unset, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn repl_set_with_no_args_shows_options() {
    // Bare `:set` lists the options and their state.
    let output = run_flux_repl(":set\n:quit\n");
    let results = repl_results(&output);
    for expected in ["+t", "+s", "optimize", "analyze"] {
        assert!(
            results.iter().any(|line| line.starts_with(expected)),
            "`:set` should list `{expected}`, stdout:\n{}",
            String::from_utf8_lossy(&output.stdout),
        );
    }
}

#[test]
fn repl_set_unknown_option_reports_without_panicking() {
    // An unknown option is reported (not a panic) and the session continues.
    let output = run_flux_repl(":set +z\n1 + 1\n:quit\n");
    assert_eq!(repl_results(&output), ["2"]);
    let combined = combined_output(&output);
    assert!(
        combined.contains("unknown option") && !combined.contains("panicked"),
        "bad `:set` must report, not panic, output:\n{combined}",
    );
}

#[test]
fn repl_script_runs_a_file_of_repl_inputs() {
    // `:script <file>` feeds each line of the file through the same dispatch as
    // typed input — declarations and expressions alike.
    let (_files, script) = repl_temp_flx("script", "let s = 3\ns * s\n1 + 1\n");
    let output = run_flux_repl(&format!(":script \"{}\"\n:quit\n", script.display()));
    assert_eq!(
        repl_results(&output),
        ["9", "2"],
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_shell_command_runs_and_inherits_stdout() {
    // `:!<cmd>` runs in the system shell with the REPL's stdout inherited, so the
    // command's output shows up in the value stream. `echo` works on cmd and sh.
    let output = run_flux_repl(":!echo flux_shell_marker_321\n:quit\n");
    assert!(
        repl_results(&output).iter().any(|line| line.contains("flux_shell_marker_321")),
        "shell output should appear on stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn repl_cd_reports_error_for_a_missing_directory() {
    // A bad `:cd` is reported (not a panic) and the session keeps going.
    let output = run_flux_repl(":cd /no/such/dir/xyz123\n1 + 1\n:quit\n");
    assert_eq!(repl_results(&output), ["2"]);
    let combined = combined_output(&output);
    assert!(
        combined.contains("cd:") && !combined.contains("panicked"),
        "a bad `:cd` must report, not panic, output:\n{combined}",
    );
}

#[test]
fn repl_edit_without_a_target_reports_usage() {
    // `:edit` with no argument and nothing loaded prints usage and launches no
    // editor (so the test never blocks), and the session continues.
    let output = run_flux_repl(":edit\n1 + 1\n:quit\n");
    assert_eq!(repl_results(&output), ["2"]);
    assert!(
        combined_output(&output).contains("usage: :edit"),
        "expected a `:edit` usage hint, output:\n{}",
        combined_output(&output),
    );
}
