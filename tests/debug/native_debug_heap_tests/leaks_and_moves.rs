use super::*;

#[test]
fn a_use_after_free_is_caught() {
    let scratch = Scratch::new("debug-heap-fault");
    // A sound build first: the fault keys its own cache entries, so the
    // faulted build below cannot be served this one's objects.
    let (stdout, _, _) = run(&scratch, FIXTURE, &["--native", "--debug-heap"], None);
    assert_eq!(stdout, "6\n");
    let (_, stderr, code) =
        run(&scratch, FIXTURE, &["--native", "--debug-heap"], Some("skip-dup:victim"));
    assert_eq!(code, 1, "{stderr}");
    assert!(
        stderr.contains(
            "use after free: copying a constructor value that was already freed \
             (allocated at numbers (tests/debug/heap/use_after_free.flx:2:5), \
             freed at main (tests/debug/heap/use_after_free.flx:16:16))"
        ),
        "{stderr}"
    );
    // Where the freed list was used again: the call that lends it to `total`.
    assert!(stderr.contains("Stack trace:"), "{stderr}");
    assert!(stderr.contains("at main (tests/debug/heap/use_after_free.flx:17:23)"), "{stderr}");
}

#[test]
fn without_the_debug_heap_the_fault_goes_unnoticed() {
    // What the debug heap is for: the plain runtime hands the freed block
    // out again, so the faulted program exits cleanly with whatever the
    // block now holds, and nothing reports the fault.
    let scratch = Scratch::new("debug-heap-unnoticed");
    let (_, stderr, code) = run(&scratch, FIXTURE, &["--native"], Some("skip-dup:victim"));
    assert_eq!(code, 0, "{stderr}");
    assert!(!stderr.contains("use after free"), "{stderr}");
}

#[test]
fn the_fixture_runs_clean_without_the_fault() {
    let scratch = Scratch::new("debug-heap-clean");
    let (stdout, stderr, code) = run(&scratch, FIXTURE, &["--native", "--debug-heap"], None);
    assert_eq!((stdout.as_str(), code), ("6\n", 0), "{stderr}");
}

/// Programs over lists, tuples, records, closures, strings, maps, reuse and
/// worker threads print the same on the debug heap: it reports no fault in
/// a sound program.
#[test]
fn programs_run_the_same_on_the_debug_heap() {
    let files: Vec<String> = [
        "examples/guide/tuples.flx",
        "examples/guide/records.flx",
        "examples/guide/higher_order_builtins.flx",
        "examples/guide/mini_database.flx",
        "examples/guide/sort_by_string_len.flx",
        "examples/guide/map_builtins.flx",
        "examples/guide/using_list_module.flx",
        "examples/async/15_concurrent_jobs.flx",
        "examples/async/18_task_closure_captures.flx",
    ]
    .map(String::from)
    .into();
    let failures = failures_in_parallel("debug-heap-programs", &files, 4, |scratch, file| {
        let (plain, _, plain_code) = run(scratch, file, &["--native"], None);
        let (heap, stderr, heap_code) = run(scratch, file, &["--native", "--debug-heap"], None);
        ((heap.as_str(), heap_code) != (plain.as_str(), plain_code)).then(|| {
            format!(
                "{file}: plain {:?} (exit {plain_code}), debug heap {:?} (exit {heap_code})\n{}",
                clip(&plain),
                clip(&heap),
                clip(&stderr)
            )
        })
    });
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A nested match rebuilding the inner list, over lists built at run time,
/// prints what the VM prints on the debug heap: Aether used to drop the
/// outer list twice on every path.
#[test]
fn a_nested_match_releases_each_list_once() {
    let scratch = Scratch::new("debug-heap-nested-match");
    let file = "tests/debug/heap/nested_match.flx";
    let (vm, _, _) = run(&scratch, file, &["--vm"], None);
    let (heap, stderr, code) = run(&scratch, file, &["--native", "--debug-heap"], None);
    assert_eq!((heap.as_str(), code), (vm.as_str(), 0), "{stderr}");
    assert_eq!(vm, "[34, 23, 12]\n[510, 405]\n[]\n");
}

/// Functions that rebuild a list, on lists built at run time that the
/// caller keeps (and on a worker thread): each prints what the VM prints on
/// the debug heap. A function inferred to borrow its list used to release
/// it, and the caller's guard released it again.
#[test]
fn kept_lists_survive_the_functions_they_are_lent_to() {
    let scratch = Scratch::new("debug-heap-shared-lists");
    for file in ["tests/debug/heap/shared_lists.flx", "tests/debug/heap/shared_lists_tasks.flx"] {
        let (vm, _, _) = run(&scratch, file, &["--vm"], None);
        let (heap, stderr, code) = run(&scratch, file, &["--native", "--debug-heap"], None);
        assert!(!vm.is_empty(), "{file}");
        assert_eq!((heap.as_str(), code), (vm.as_str(), 0), "{file}:\n{stderr}");
    }
}

/// A variable lent and given in the same call: the given reference is the
/// callee's to free, so the lent one needs a reference of its own.
#[test]
fn a_list_lent_and_given_in_one_call_survives_the_call() {
    let scratch = Scratch::new("debug-heap-lent-and-owned");
    let file = "tests/debug/heap/lent_and_owned.flx";
    let (vm, _, _) = run(&scratch, file, &["--vm"], None);
    assert_eq!(vm, "3\n");
    let (heap, stderr, code) = run(&scratch, file, &["--native", "--debug-heap"], None);
    assert_eq!((heap.as_str(), code), ("3\n", 0), "{stderr}");
}

const LEAK_FIXTURE: &str = "tests/debug/heap/leak.flx";

/// The lines of a leak report that name an allocation site.
fn site_lines(stderr: &str) -> Vec<&str> {
    stderr.lines().filter(|line| line.contains(" allocated at ")).collect()
}

#[test]
fn a_leak_is_reported_at_exit_with_its_allocation_site() {
    let scratch = Scratch::new("debug-heap-leak");
    let (_, stderr, code) =
        run(&scratch, LEAK_FIXTURE, &["--native", "--leak-check"], Some("skip-drop:leaked"));
    assert_eq!(code, 3, "{stderr}");
    assert!(stderr.contains("leak: 3 blocks still live at exit"), "{stderr}");
    let sites = site_lines(&stderr);
    assert_eq!(sites.len(), 3, "{stderr}");
    for line in sites {
        assert_eq!(
            line,
            "  a constructor value (24 bytes) allocated at numbers \
             (tests/debug/heap/leak.flx:5:5)",
            "{stderr}"
        );
    }
}

#[test]
fn a_leak_report_names_at_most_ten_sites() {
    let scratch = Scratch::new("debug-heap-leak-many");
    let (_, stderr, code) =
        run(&scratch, LEAK_FIXTURE, &["--native", "--leak-check"], Some("skip-drop:many"));
    assert_eq!(code, 3, "{stderr}");
    assert!(stderr.contains("leak: 12 blocks still live at exit"), "{stderr}");
    assert_eq!(site_lines(&stderr).len(), 10, "{stderr}");
    assert!(stderr.contains("\n  and 2 more\n"), "{stderr}");
}

#[test]
fn the_leak_fixture_frees_everything_without_the_fault() {
    let scratch = Scratch::new("debug-heap-leak-clean");
    let (_, stderr, code) = run(&scratch, LEAK_FIXTURE, &["--native", "--leak-check"], None);
    assert_eq!(code, 0, "{stderr}");
    assert!(!stderr.contains("leak:"), "{stderr}");
}

#[test]
fn a_value_passed_by_ownership_is_freed_by_its_callee() {
    let scratch = Scratch::new("debug-heap-owned-arg");
    let (stdout, stderr, code) =
        run(&scratch, "tests/debug/heap/owned_arg.flx", &["--native", "--leak-check"], None);
    assert_eq!((stdout.as_str(), code), ("", 0), "{stderr}");
    assert!(!stderr.contains("leak:"), "{stderr}");
}

/// KI-160: an interpolation's part strings and the joins between them are
/// released, so only its result is left to its owner.
#[test]
fn an_interpolation_frees_its_parts_and_joins() {
    let scratch = Scratch::new("debug-heap-interpolated-parts");
    let (stdout, stderr, code) = run(
        &scratch,
        "tests/debug/heap/interpolated_parts.flx",
        &["--native", "--leak-check"],
        None,
    );
    assert_eq!((stdout.as_str(), code), ("", 0), "{stderr}");
    assert!(!stderr.contains("leak:"), "{stderr}");
}

/// KI-119: the default handler around `main` answers each `print` with `()`,
/// a tuple of no fields, which stayed live at exit. It is a constant, so it
/// is a static object, and a program that prints leaves nothing.
#[test]
fn a_program_that_prints_leaves_nothing_live_ki119() {
    let scratch = Scratch::new("debug-heap-print-ki119");
    for (name, program, printed) in [
        ("one_print", "fn main() {\n    print(1)\n}\n", "1\n"),
        (
            "three_prints",
            "fn main() with IO {\n    print(1)\n    print(2)\n    print(3)\n}\n",
            "1\n2\n3\n",
        ),
    ] {
        let path = scratch.write(&format!("{name}.flx"), program);
        let (stdout, stderr, code) =
            run(&scratch, &path.to_string_lossy(), &["--native", "--leak-check"], None);
        assert_eq!((stdout.as_str(), code), (printed, 0), "{name}: {stderr}");
        assert!(!stderr.contains("leak:"), "{name}: {stderr}");
    }
}

/// KI-161: a Float literal used as an operand or an argument is owned by no
/// binder, so nothing drops it; boxed at each use, it leaked once per use,
/// and once per iteration in a loop. A constructor of constants holding one
/// is checked too, since it becomes a constant itself.
#[test]
fn float_literals_leave_nothing_live_ki161() {
    let scratch = Scratch::new("debug-heap-float-literals-ki161");
    for (name, program) in [
        (
            "operand_and_argument",
            "fn half(x: Float) -> Float {\n    x * 0.5\n}\n\n\
             fn main() {\n    let h = half(3.0)\n    if h > 1.0 { 0 } else { 1 }\n}\n",
        ),
        ("let_used_twice", "fn main() {\n    let a = 3.14\n    print(a + a)\n}\n"),
        (
            "in_a_loop",
            "fn go(n: Int, acc: Float) -> Float {\n    \
             if n == 0 { acc } else { go(n - 1, acc + 0.5) }\n}\n\n\
             fn main() {\n    print(go(100, 0.0))\n}\n",
        ),
        (
            "in_a_constructor",
            "fn pick(o: Option<Float>) -> Float {\n    match o {\n        \
             Some(x) -> x,\n        None -> -1.0,\n    }\n}\n\n\
             fn main() {\n    print(pick(Some(1.5)))\n}\n",
        ),
    ] {
        let path = scratch.write(&format!("{name}.flx"), program);
        let path = path.to_string_lossy();
        let (vm, _, _) = run(&scratch, &path, &["--vm"], None);
        let (stdout, stderr, code) = run(&scratch, &path, &["--native", "--leak-check"], None);
        assert_eq!((stdout.as_str(), code), (vm.as_str(), 0), "{name}: {stderr}");
        assert!(!stderr.contains("leak:"), "{name}: {stderr}");
    }
}

/// A value read only in an `if` condition is released when the `if` is
/// passed as an argument (KI-199). So is the owner when one arm gives it away
/// or copies a field of it; the debug heap faults if it goes too soon.
#[test]
fn a_value_read_only_in_an_if_argument_is_released_ki199() {
    let scratch = Scratch::new("debug-heap-if-argument-ki199");
    let nums = "fn nums(n: Int) -> List<Int> {\n    [n, n + 1, n + 2]\n}\n\n";
    let leak_free = [
        (
            "int_arms",
            format!(
                "{nums}fn keep_int(n: Int) -> Int {{\n    n\n}}\n\n\
                 fn main() {{\n    let xs = nums(1)\n    \
                 keep_int(if len(xs) > 1 {{ 0 }} else {{ 1 }})\n}}\n"
            ),
        ),
        (
            "string_arms",
            format!(
                "{nums}fn main() {{\n    let xs = nums(1)\n    \
                 print(if len(xs) > 1 {{ \"a\" }} else {{ \"b\" }})\n}}\n"
            ),
        ),
        (
            "per_call",
            format!(
                "{nums}fn check(n: Int) with IO {{\n    let xs = nums(n)\n    \
                 print(if len(xs) > 1 {{ \"a\" }} else {{ \"b\" }})\n}}\n\n\
                 fn go(n: Int) with IO {{\n    if n > 0 {{\n        check(n)\n        \
                 go(n - 1)\n    }}\n}}\n\n\
                 fn main() with IO {{\n    go(10)\n}}\n"
            ),
        ),
        (
            "arm_returns_owner",
            format!(
                "{nums}fn total(ys: List<Int>) -> Int {{\n    match ys {{\n        \
                 [h | t] -> h + total(t),\n        _ -> 0,\n    }}\n}}\n\n\
                 fn main() {{\n    let xs = nums(1)\n    \
                 print(total(if len(xs) > 1 {{ xs }} else {{ [] }}))\n}}\n"
            ),
        ),
        (
            "arm_returns_field",
            "fn name(n: Int) -> Option<String> {\n    \
             if n > 0 { Some(\"n=#{n}\") } else { None }\n}\n\n\
             fn main() {\n    let o = name(3)\n    print(match o {\n        \
             Some(s) -> s,\n        None -> \"none\",\n    })\n}\n"
                .to_string(),
        ),
    ];
    for (name, program) in &leak_free {
        let path = scratch.write(&format!("{name}.flx"), program);
        let path = path.to_string_lossy();
        let (vm, _, _) = run(&scratch, &path, &["--vm"], None);
        let (stdout, stderr, code) = run(&scratch, &path, &["--native", "--leak-check"], None);
        assert_eq!((stdout.as_str(), code), (vm.as_str(), 0), "{name}: {stderr}");
        assert!(!stderr.contains("leak:"), "{name}: {stderr}");
    }
}

/// The result of a runtime primop that makes a new value, or counts its
/// argument again, is released: when bound, and when the value it was made
/// from is released after it (KI-201). `safe_div` and `safe_mod` reach Aether
/// as runtime calls rather than primops.
#[test]
fn the_results_of_runtime_primops_are_released_ki201() {
    let scratch = Scratch::new("debug-heap-runtime-primops-ki201");
    for (name, program) in [
        (
            "array_push_printed",
            r#"fn main() {
    let arr = [|1, 2, 3|]
    print(array_push(arr, 4))
    print(len(arr))
}
"#,
        ),
        (
            "array_push_of_a_let",
            r#"fn main() {
    let b = array_push([|1, 2, 3|], 4)
    print(len(b))
}
"#,
        ),
        (
            "array_concat",
            r#"fn main() {
    let c = array_concat([|1|], [|2, 3|])
    print(c)
}
"#,
        ),
        (
            "array_slice",
            r#"fn main() {
    let a = [|1, 2, 3, 4|]
    let s = array_slice(a, 1, 3)
    print(s)
}
"#,
        ),
        (
            "array_set",
            r#"fn main() {
    let a = [|1, 2, 3|]
    let b = array_set(a, 0, 9)
    print(b)
    print(a)
}
"#,
        ),
        (
            "safe_div_of_a_neg",
            r#"fn main() {
    print(safe_div(-15, 3))
}
"#,
        ),
        (
            "safe_div_let_then_match",
            r#"fn main() {
    let result = safe_div(100, 4)
    match result {
        Some(v) -> print(v),
        None -> print("none"),
    }
}
"#,
        ),
        (
            "safe_mod",
            r#"fn main() {
    let r = safe_mod(-7, 3)
    print(r)
}
"#,
        ),
        (
            "substring_compared",
            r#"fn starts(s: String, p: String) -> Bool {
    substring(s, 0, len(p)) == p
}

fn main() {
    print(starts("hello", "he"))
}
"#,
        ),
        (
            "substring_pushed",
            r#"fn main() {
    let s = "abc"
    let a = array_push([||], substring(s, 0, 1))
    print(a)
}
"#,
        ),
        (
            "string_slice",
            r#"fn main() {
    let src = "hello" + " world"
    let s = string_slice(src, 0, 5)
    print(s)
}
"#,
        ),
        (
            "split",
            r#"fn main() {
    let src = "a,b" + ",c"
    let parts = split(src, ",")
    print(parts)
}
"#,
        ),
        (
            "trim",
            r#"fn main() {
    let src = "  hi" + "  "
    let t = trim(src)
    print(t)
}
"#,
        ),
        (
            "upper",
            r#"fn main() {
    let src = "h" + "i"
    let u = upper(src)
    print(u)
}
"#,
        ),
        (
            "lower",
            r#"fn main() {
    let src = "H" + "I"
    let l = lower(src)
    print(l)
}
"#,
        ),
        (
            "replace",
            r#"fn main() {
    let src = "a-b" + "-c"
    let r = replace(src, "-", "+")
    print(r)
}
"#,
        ),
    ] {
        let path = scratch.write(&format!("{name}.flx"), program);
        let path = path.to_string_lossy();
        let (vm, _, _) = run(&scratch, &path, &["--vm"], None);
        let (stdout, stderr, code) = run(&scratch, &path, &["--native", "--leak-check"], None);
        assert_eq!((stdout.as_str(), code), (vm.as_str(), 0), "{name}: {stderr}");
        assert!(!stderr.contains("leak:"), "{name}: {stderr}");
    }
}

/// Reading a member of a record literal, a map, releases the `Some` the
/// lookup made: one was left per read (KI-203).
#[test]
fn a_value_read_from_a_record_literal_releases_its_option_ki203() {
    let scratch = Scratch::new("debug-heap-record-member-ki203");
    for (name, program) in [
        (
            "call_member",
            r#"fn main() {
    let m = { "square": fn(x) { x * x } }
    print(m.square(5))
}
"#,
        ),
        (
            "call_member_twice",
            r#"fn main() {
    let m = { "square": fn(x) { x * x } }
    print(m.square(5))
    print(m.square(6))
}
"#,
        ),
        (
            "two_members",
            r#"fn main() {
    let m = { "inc": fn(x) { x + 1 }, "dec": fn(x) { x - 1 } }
    print(m.inc(5))
    print(m.dec(5))
}
"#,
        ),
        (
            "member_in_a_loop",
            r#"fn go(n: Int, acc: Int) -> Int {
    let m = { "f": fn(x) { x + 1 } }
    if n == 0 { acc } else { go(n - 1, m.f(acc)) }
}

fn main() {
    print(go(10, 0))
}
"#,
        ),
    ] {
        let path = scratch.write(&format!("{name}.flx"), program);
        let path = path.to_string_lossy();
        let (vm, _, _) = run(&scratch, &path, &["--vm"], None);
        let (stdout, stderr, code) = run(&scratch, &path, &["--native", "--leak-check"], None);
        assert_eq!((stdout.as_str(), code), (vm.as_str(), 0), "{name}: {stderr}");
        assert!(!stderr.contains("leak:"), "{name}: {stderr}");
    }
}

/// A fiber waiting on a timeout, `race` or `both` whose body panics is woken
/// with the error and released: its continuation and the effect handlers its
/// context holds were left live (KI-206). A body that returns and a timer
/// that fires check that nothing is freed early.
#[test]
fn a_fiber_woken_with_an_error_releases_its_continuation_ki206() {
    let scratch = Scratch::new("debug-heap-fiber-error-wake-ki206");
    for (name, program) in [
        (
            "timeout_body_panics",
            r#"import Flow.Async exposing (..)

fn body() -> Int with Async {
    panic("boom")
}

fn main() with IO {
    print(run_async(fn() { timeout_result(100, body) }))
}
"#,
        ),
        (
            "timeout_body_panics_three_times",
            r#"import Flow.Async exposing (..)

fn body() -> Int with Async {
    panic("boom")
}

fn main() with IO {
    print(run_async(fn() { timeout_result(100, body) }))
    print(run_async(fn() { timeout_result(100, body) }))
    print(run_async(fn() { timeout_result(100, body) }))
}
"#,
        ),
        (
            "race_child_panics",
            r#"import Flow.Async exposing (..)

fn bad() -> Int with Async {
    panic("boom")
}

fn slow() -> Int with Async {
    sleep(50)
    1
}

fn main() with IO {
    print(run_async(fn() { try(fn() { race(bad, slow) }) }))
}
"#,
        ),
        (
            "both_child_panics",
            r#"import Flow.Async exposing (..)

fn bad() -> Int with Async {
    panic("boom")
}

fn good() -> Int with Async {
    1
}

fn main() with IO {
    print(run_async(fn() { try(fn() { both(good, bad) }) }))
}
"#,
        ),
        (
            "timeout_body_returns",
            r#"import Flow.Async exposing (..)

fn body() -> Int with Async {
    7
}

fn main() with IO {
    print(run_async(fn() { timeout_result(100, body) }))
}
"#,
        ),
        (
            "timeout_timer_fires",
            r#"import Flow.Async exposing (..)

fn body() -> Int with Async {
    sleep(50)
    7
}

fn main() with IO {
    print(run_async(fn() { timeout_result(1, body) }))
}
"#,
        ),
    ] {
        let path = scratch.write(&format!("{name}.flx"), program);
        let path = path.to_string_lossy();
        let (vm, _, _) = run(&scratch, &path, &["--vm"], None);
        let (stdout, stderr, code) = run(&scratch, &path, &["--native", "--leak-check"], None);
        assert_eq!((stdout.as_str(), code), (vm.as_str(), 0), "{name}: {stderr}");
        assert!(!stderr.contains("leak:"), "{name}: {stderr}");
    }
}

/// `--debug-heap` alone does not report leaks: nearly every program still
/// leaks (KI-119, KI-121).
#[test]
fn the_debug_heap_alone_does_not_report_leaks() {
    let scratch = Scratch::new("debug-heap-leak-off");
    let (_, stderr, code) =
        run(&scratch, LEAK_FIXTURE, &["--native", "--debug-heap"], Some("skip-drop:leaked"));
    assert_eq!(code, 0, "{stderr}");
    assert!(!stderr.contains("leak:"), "{stderr}");
}

#[test]
fn without_the_debug_heap_a_leak_goes_unnoticed() {
    let scratch = Scratch::new("debug-heap-leak-unnoticed");
    let (_, stderr, code) = run(&scratch, LEAK_FIXTURE, &["--native"], Some("skip-drop:leaked"));
    assert_eq!(code, 0, "{stderr}");
}

#[test]
fn the_leak_check_needs_the_native_backend() {
    let scratch = Scratch::new("debug-heap-leak-vm");
    let (_, stderr, code) = run(&scratch, LEAK_FIXTURE, &["--vm", "--leak-check"], None);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("Error: --leak-check applies to a program run or built with --native."),
        "{stderr}"
    );
}

#[test]
fn the_debug_heap_needs_the_native_backend() {
    let scratch = Scratch::new("debug-heap-vm");
    for args in [&["--vm", "--debug-heap"][..], &["--debug-heap"][..]] {
        let (_, stderr, code) = run(&scratch, FIXTURE, args, None);
        assert_eq!(code, 1);
        assert!(
            stderr.contains("Error: --debug-heap applies to a program run or built with --native."),
            "{stderr}"
        );
    }
}

#[test]
fn debug_heap_binaries_are_cached_apart() {
    let scratch = Scratch::new("debug-heap-cache");
    run(&scratch, FIXTURE, &["--native"], None);
    run(&scratch, FIXTURE, &["--native", "--debug-heap"], None);
    let binaries: Vec<String> = walk(scratch.path())
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_string();
            name.ends_with(".bin").then_some(name)
        })
        .collect();
    assert!(binaries.contains(&"use_after_free.bin".to_string()), "{binaries:?}");
    assert!(binaries.contains(&"use_after_free.debug-heap.bin".to_string()), "{binaries:?}");
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}
#[test]
fn heap_stats_are_reported_only_when_asked() {
    let scratch = Scratch::new("debug-heap-stats");
    let file = "tests/debug/heap/owned_arg.flx";
    let args = ["--native", "--debug-heap"];
    let (_, stderr, code) = run_env(&scratch, file, &args, &[("FLUX_HEAP_STATS", "1")]);
    assert_eq!(code, 0, "{stderr}");
    let (allocs, frees, live) = heap_stats(&stderr).unwrap_or_else(|| panic!("{stderr}"));
    assert!(allocs > 0, "{stderr}");
    assert_eq!((frees, live), (allocs, 0), "{stderr}");

    // A leak exit forwards all of stderr, so these see what the runtime
    // printed, not what the driver kept on a normal exit.
    let leak_args = ["--native", "--leak-check"];
    let leaked = ("FLUX_AETHER_FAULT", "skip-drop:leaked");
    let (_, stderr, code) =
        run_env(&scratch, LEAK_FIXTURE, &leak_args, &[leaked, ("FLUX_HEAP_STATS", "1")]);
    assert_eq!(code, 3, "{stderr}");
    assert_eq!(heap_stats(&stderr).map(|(_, _, live)| live), Some(3), "{stderr}");
    assert!(stderr.find("heap: ") < stderr.find("leak: "), "{stderr}");
    let (_, stderr, code) =
        run_env(&scratch, LEAK_FIXTURE, &leak_args, &[leaked, ("FLUX_HEAP_STATS", "0")]);
    assert_eq!(code, 3, "{stderr}");
    assert!(!stderr.contains("heap:"), "{stderr}");
}

const OWNED_MOVES: &str = "tests/debug/heap/owned_moves";

/// Proposal 0189's hazards V1, V2, V4, V6 and V7, and the step-1 review's
/// repros: places a value is handed out or shared without being counted. Each
/// fixture prints on the debug heap what it prints on the VM: a freed value
/// read back shows up as poison there.
///
/// The fixtures are split over four tests, which nextest runs side by side.
/// Each builds on two workers, each with a scratch directory and so a cache
/// of its own: the native cache is not safe for two builds writing it at
/// once, and one worker building them all takes minutes.
fn owned_moves_part(part: usize) {
    let files: Vec<String> = owned_move_fixtures()
        .into_iter()
        .enumerate()
        .filter(|(index, _)| index % 4 == part)
        .map(|(_, name)| name)
        .collect();
    assert!(!files.is_empty(), "no fixtures in part {part}");
    let scratch = format!("debug-heap-owned-moves-{part}");
    // CI runners have three cores; more builds there slow the timed tests
    // running beside these.
    let workers = if std::env::var_os("CI").is_some() { 1 } else { 2 };
    let failures = failures_in_parallel(&scratch, &files, workers, owned_move_failure);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn owned_moves_print_what_the_vm_prints_part_1() {
    owned_moves_part(0)
}

#[test]
fn owned_moves_print_what_the_vm_prints_part_2() {
    owned_moves_part(1)
}

#[test]
fn owned_moves_print_what_the_vm_prints_part_3() {
    owned_moves_part(2)
}

#[test]
fn owned_moves_print_what_the_vm_prints_part_4() {
    owned_moves_part(3)
}

/// The fixtures of `OWNED_MOVES`, sorted.
fn owned_move_fixtures() -> Vec<String> {
    let mut files: Vec<String> = std::fs::read_dir(workspace_root().join(OWNED_MOVES))
        .expect("owned_moves fixtures")
        .filter_map(|entry| {
            entry.ok().map(|entry| entry.file_name().to_string_lossy().into_owned())
        })
        .filter(|name| Path::new(name).extension().is_some_and(|ext| ext == "flx"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no fixtures in {OWNED_MOVES}");
    files
}

/// How fixture `name` fails to print on the debug heap what the VM prints.
fn owned_move_failure(scratch: &Scratch, name: &str) -> Option<String> {
    let file = format!("{OWNED_MOVES}/{name}");
    let (vm, _, vm_code) = run(scratch, &file, &["--vm"], None);
    let (heap, stderr, code) = run(scratch, &file, &["--native", "--debug-heap"], None);
    (vm.is_empty() || vm_code != 0 || (heap.as_str(), code) != (vm.as_str(), 0)).then(|| {
        let (heap, stderr) = (clip(&heap), clip(&stderr));
        format!("{name}: vm {vm:?} (exit {vm_code}), debug heap {heap:?} (exit {code})\n{stderr}")
    })
}
