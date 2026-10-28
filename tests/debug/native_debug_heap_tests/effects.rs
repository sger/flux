use super::*;

/// Programs that build a value with `n` fields: a constructor, a tuple, and
/// a closure that captures `n` values; each with what it prints.
fn wide_values(n: usize) -> [(&'static str, String, String); 3] {
    let vals = (0..n).map(|i| format!("n + {i}")).collect::<Vec<_>>().join(", ");
    let pattern =
        std::iter::once("a").chain(std::iter::repeat_n("_", n - 1)).collect::<Vec<_>>().join(", ");
    let ints = vec!["Int"; n].join(", ");
    // `keep` is recursive, so it is never inlined: the closure captures each
    // value, where it would copy a cheap `n + i` into its own body.
    let lets = (0..n).map(|i| format!("    let v{i} = keep(n + {i})\n")).collect::<String>();
    let sum = (0..n).map(|i| format!("v{i}")).collect::<Vec<_>>().join(" + ");
    [
        (
            "constructor",
            format!(
                "type Wide = Wide({ints})\n\nfn first(w) {{ match w {{ Wide({pattern}) -> a }} }}\n\n\
                 fn main() {{\n    let n = len([1, 2])\n    print(first(Wide({vals})))\n}}\n"
            ),
            "2".to_string(),
        ),
        (
            "tuple",
            format!(
                "fn first(t) {{ t.0 }}\n\n\
                 fn main() {{\n    let n = len([1, 2])\n    print(first(({vals})))\n}}\n"
            ),
            "2".to_string(),
        ),
        (
            "closure",
            format!(
                "fn keep(k) {{ if k < 0 {{ keep(k + 1) }} else {{ k }} }}\n\n\
                 fn make(n) {{\n{lets}    \\x -> x + {sum}\n}}\n\n\
                 fn main() {{\n    let f = make(len([1, 2]))\n    print(f(1))\n}}\n"
            ),
            // 1 + the sum of 2 + i for i below n.
            (1 + 2 * n + n * (n - 1) / 2).to_string(),
        ),
    ]
}

/// A heap object counts the fields its release walks in one byte, and the
/// VM's instructions count fields and captures in one byte too, so a value
/// with more than 255 is rejected before either backend sees it. Natively it
/// used to build and leak every field past the 255th; the VM failed at run
/// time.
#[test]
fn a_value_with_more_than_255_fields_is_rejected() {
    let scratch = Scratch::new("debug-heap-wide-values");
    let mut failures = Vec::new();
    for (name, program, _) in wide_values(256) {
        let path = scratch.write(&format!("wide_{name}.flx"), &program);
        let file = path.to_string_lossy();
        for (backend, args) in [("native", &["--native"][..]), ("vm", &[][..])] {
            let (_, stderr, code) = run(&scratch, &file, args, None);
            if code == 0 || !stderr.contains("E087") {
                failures.push(format!("{name} on {backend}: exit {code}\n{}", clip(&stderr)));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The largest values that fit, 255 fields or captures, still build and run
/// on both backends.
#[test]
fn a_value_with_255_fields_runs() {
    let scratch = Scratch::new("debug-heap-widest-values");
    let mut failures = Vec::new();
    for (name, program, printed) in wide_values(255) {
        let path = scratch.write(&format!("widest_{name}.flx"), &program);
        let file = path.to_string_lossy();
        for (backend, args) in [("native", &["--native"][..]), ("vm", &[][..])] {
            let (stdout, stderr, code) = run(&scratch, &file, args, None);
            if code != 0 || stdout.lines().last() != Some(printed.as_str()) {
                failures.push(format!(
                    "{name} on {backend}: exit {code}, printed {stdout:?}, want {printed}\n{}",
                    clip(&stderr)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const PERFORM_PER_TURN: &str = r"effect Ask {
    ask : (ARG) -> Int
}

fn count(n, total) -> Int with Ask {
    if n == 0 { total } else {
        let k = perform Ask.ask(SENT)
        count(n - 1, total + k)
    }
}

fn run() -> Int {
    count(500, 0) handle Ask {
        ask(resume, x) -> resume(READ)
    }
}

fn main() with IO {
    print(to_string(run()))
}
";

/// A perform gives its argument to the clause. The same loop sending a
/// fresh list or its length leaves the same blocks behind: a perform that
/// copied its argument kept every list it sent.
#[test]
fn a_perform_takes_the_argument_it_is_given() {
    let scratch = Scratch::new("debug-heap-perform-argument");
    let program = |arg: &str, sent: &str, read: &str| {
        PERFORM_PER_TURN.replace("ARG", arg).replace("SENT", sent).replace("READ", read)
    };
    let (_, _, list) =
        stats_of(&scratch, "list.flx", &program("List<Int>", "[n, n + 1, n + 2]", "len(x)"));
    let (_, _, int) = stats_of(&scratch, "int.flx", &program("Int", "len([n, n + 1, n + 2])", "x"));
    assert_eq!(list, int, "live blocks: list sent vs its length sent");
}

const RESUME_PER_TURN: &str = r"effect Ask {
    ask : (Int) -> Int
}

fn get(n) -> Int with Ask {
    perform Ask.ask(n) + 1
}

fn count(n, total) -> Int with Ask {
    if n == 0 { total } else {
        let k = PERFORM
        count(n - 1, total + k)
    }
}

fn run() -> Int {
    count(SIZE, 0) handle Ask {
        ask(resume, x) -> CLAUSE
    }
}

fn main() with IO {
    print(to_string(run()))
}
";

/// The continuation a clause resumes is released once the clause is done:
/// one captured frame, two frames joined into one resume, and a resume
/// that hands the clause back its later work. Nothing heap-allocated lives
/// across the perform, so each turn leaves nothing behind.
#[test]
fn a_resumed_continuation_is_released() {
    let scratch = Scratch::new("debug-heap-resume-continuation");
    let failures: Vec<String> = [
        ("one_frame", "perform Ask.ask(n)", "resume(x)"),
        ("two_frames", "get(n)", "resume(x)"),
        ("later_work", "perform Ask.ask(n)", "resume(x) + 0"),
    ]
    .into_iter()
    .filter_map(|(name, perform, clause)| {
        let program = RESUME_PER_TURN.replace("PERFORM", perform).replace("CLAUSE", clause);
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const READ_AFTER_PERFORM: &str = r"effect Ask {
    ask : (Int) -> Int
}

fn get(n) -> Int with Ask {
    perform Ask.ask(n) + 1
}

fn numbers(n) {
    [n, n + 1, n + 2]
}

fn count(n, total) -> Int with Ask {
    if n == 0 { total } else {
        let xs = numbers(n)
        let k = PERFORM
        count(n - 1, total + k + len(xs))
    }
}

fn run() -> Int {
    count(SIZE, 0) handle Ask {
        ask(resume, x) -> CLAUSE
    }
}

fn main() with IO {
    print(to_string(run()))
}
";

/// Live blocks of `program` at 3 turns and at 4, for a clause that resumes
/// twice: each turn doubles the work, so the loop stays a few turns deep.
fn live_at_three_and_four(scratch: &Scratch, name: &str, program: &str) -> (u64, u64) {
    let at = |n: u64| {
        let source = program.replace("SIZE", &n.to_string());
        stats_of(scratch, &format!("{name}_{n}.flx"), &source).2
    };
    (at(3), at(4))
}

/// A list made before a perform and read after it is released after that
/// read, however the clause resumes. A clause that resumes twice runs the
/// rest of the turn twice, and each run releases the reference it was
/// given.
#[test]
fn a_value_read_after_a_perform_is_released() {
    let scratch = Scratch::new("debug-heap-read-after-perform");
    let program = |clause: &str| {
        READ_AFTER_PERFORM.replace("PERFORM", "perform Ask.ask(n)").replace("CLAUSE", clause)
    };
    let mut failures: Vec<String> = [("one_frame", "resume(x)"), ("later_work", "resume(x) + 0")]
        .into_iter()
        .filter_map(|(name, clause)| {
            let (_, _, live) = per_thousand_cells(&scratch, name, &program(clause));
            (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
        })
        .collect();
    let (three, four) =
        live_at_three_and_four(&scratch, "resumed_twice", &program("resume(x) + resume(x)"));
    if three != four {
        failures.push(format!("resumed_twice: {three} blocks live at 3 turns, {four} at 4"));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const ABORT_PER_TURN: &str = r"effect Ask {
    ask : (Int) -> Int
}

fn get(n) -> Int with Ask {
    perform Ask.ask(n) + 1
}

fn numbers(n) {
    [n, n + 1, n + 2]
}

fn attempt(n) -> Int with Ask {
    let xs = numbers(n)
    BODY
}

fn try(n) -> Int {
    attempt(n) handle Ask {
        ask(resume, x) -> 0
    }
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + try(n)) }
}

fn main() with IO {
    print(to_string(count(500, 0)))
}
";

const BORROWED_ACROSS_A_YIELD: &str = r"effect Ask {
    ask : (Int) -> Int
}

fn get(n) -> Int with Ask {
    perform Ask.ask(n) + 1
}

fn numbers(n) {
    [n, n + 1, n + 2]
}

fn walk(xs, n) -> Int with Ask {
    let k = get(n)
    k + len(xs)
}

fn count(n, total) -> Int with Ask {
    if n == 0 { total } else {
        let xs = numbers(n)
        let r = walk(xs, n)
        count(n - 1, total + r + len(xs))
    }
}

fn run() -> Int {
    count(SIZE, 0) handle Ask {
        ask(resume, x) -> resume(x) + resume(x)
    }
}

fn main() with IO {
    print(to_string(run()))
}
";

/// A call that performs inside it yields through the caller's frame, which
/// becomes a continuation. The frame's own values move into it and each
/// resume copies only those, so a list the frame made is released after
/// its last read however the clause resumes, and a list it only borrows is
/// never copied. A clause that never resumes frees the continuation, and
/// with it the frame's list: it leaves no more behind than a frame that
/// released the list before the call.
#[test]
fn a_value_live_across_a_yielding_call_is_released() {
    let scratch = Scratch::new("debug-heap-across-a-yield");
    let program =
        |clause: &str| READ_AFTER_PERFORM.replace("PERFORM", "get(n)").replace("CLAUSE", clause);
    let mut failures: Vec<String> = [("two_frames", "resume(x)"), ("later_work", "resume(x) + 0")]
        .into_iter()
        .filter_map(|(name, clause)| {
            let (_, _, live) = per_thousand_cells(&scratch, name, &program(clause));
            (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
        })
        .collect();
    for (name, program) in [
        ("resumed_twice", program("resume(x) + resume(x)")),
        ("borrowed", BORROWED_ACROSS_A_YIELD.to_string()),
    ] {
        let (three, four) = live_at_three_and_four(&scratch, name, &program);
        if three != four {
            failures.push(format!("{name}: {three} blocks live at 3 turns, {four} at 4"));
        }
    }
    let abort = |body: &str| ABORT_PER_TURN.replace("BODY", body);
    let (_, _, kept) =
        stats_of(&scratch, "abort_kept.flx", &abort("let k = get(n)\n    k + len(xs)"));
    let (_, _, released) =
        stats_of(&scratch, "abort_released.flx", &abort("let m = len(xs)\n    get(n) + m"));
    if kept != released {
        failures.push(format!(
            "abort: {kept} blocks live with the list kept across the call, {released} without"
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const HANDLE_PER_TURN: &str = r"effect Ask {
    ask : (Int) -> Int
}

effect Counter {
    next : () -> Int
}

fn get(n) -> Int with Ask {
    perform Ask.ask(n)
}

fn count_twice() -> Int with Counter {
    let a = perform Counter.next()
    a + perform Counter.next()
}

fn numbers(n) {
    [n, n + 1, n + 2]
}

fn attempt(n) -> Int {
    HANDLE
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + attempt(n)) }
}

fn main() with IO {
    print(to_string(count(SIZE, 0)))
}
";

/// A `handle` releases what it installed once its body is done: the clause,
/// with what the clause captured, and the handler chain it made. A clause
/// with state gives the next one to `resume`, which replaces it, and can
/// still read the state it was called with afterwards.
#[test]
fn a_handle_releases_its_clause_and_state() {
    let scratch = Scratch::new("debug-heap-handle-per-turn");
    let failures: Vec<String> = [
        (
            "plain",
            "get(n) handle Ask {\n        ask(resume, x) -> resume(x + 1)\n    }",
        ),
        (
            "captures",
            "let xs = numbers(n)\n    get(n) handle Ask {\n        ask(resume, x) -> resume(x + len(xs))\n    }",
        ),
        (
            "state",
            "count_twice() handle Counter(numbers(n)) {\n        next(resume, state) -> resume(len(state), [len(state) | state])\n    }",
        ),
        (
            "state_read_after_resume",
            "count_twice() handle Counter(numbers(n)) {\n        next(resume, state) -> resume(1, [0]) + len(state)\n    }",
        ),
    ]
    .into_iter()
    .filter_map(|(name, handle)| {
        let program = HANDLE_PER_TURN.replace("HANDLE", handle);
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
