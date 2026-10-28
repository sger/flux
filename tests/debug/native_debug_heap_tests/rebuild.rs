use super::*;

/// Blocks a program allocates on the debug heap. `source` is written to the
/// scratch directory, so one shape can be measured at two sizes and the
/// blocks every program allocates (KI-119) cancel out.
fn allocs_of(scratch: &Scratch, name: &str, source: &str) -> u64 {
    stats_of(scratch, name, source).0
}

const LINEAR_BUMP: &str = r"fn build(n, acc) {
    if n == 0 { acc } else { build(n - 1, [n | acc]) }
}

fn bump(xs) {
    match xs {
        [h | t] -> [h + 1 | bump(t)],
        _ -> []
    }
}

fn main() {
    let ys = bump(build(SIZE, []))
    print(len(ys))
}
";

/// `source` with a guard on the rebuilding arm, which lowers through the
/// pattern-check chain instead of the multi-arm match.
fn guarded(source: &str) -> String {
    let out = source.replace("[h | t] ->", "[h | t] if h >= 0 ->");
    assert_ne!(out, source, "guarded: no rebuilding arm");
    out
}

/// `source`, then the rebuilt list released. `discard` owns its argument and
/// releases it there, so the release happens before the program's end and
/// nothing else holds the list.
fn released(source: &str) -> String {
    let body = source.replace(
        "    let ys = bump(build(SIZE, []))\n    print(len(ys))",
        "    let ys = bump(build(SIZE, []))\n    let d = discard(ys, false)\n    print(len(d))",
    );
    assert_ne!(body, source, "released: no `main` to change");
    body + "\nfn discard(xs, keep) {\n    if keep { xs } else { [] }\n}\n"
}

/// A list built and then rebuilt cell by cell, with nothing else holding it.
/// Building takes one block per cell, and the rebuild reuses each cell in
/// place. A match that copied the fields it binds made every tail shared, so
/// the rebuild took a new block per cell (0189 F3-4).
#[test]
fn a_linear_rebuild_allocates_once_per_cell() {
    let scratch = Scratch::new("debug-heap-linear-bump");
    for (name, program) in
        [("bump", LINEAR_BUMP.to_string()), ("guarded_bump", guarded(LINEAR_BUMP))]
    {
        let (allocs, _, _) = per_thousand_cells(&scratch, name, &program);
        assert_eq!(allocs, 1000, "{name}: blocks per 1000 cells");
    }
}

/// Releasing the rebuilt list frees every cell: none is left behind by a copy
/// the match made and nothing released (0189 F3-4).
#[test]
fn a_rebuilt_list_released_leaves_nothing_behind() {
    let scratch = Scratch::new("debug-heap-linear-bump-released");
    for (name, program) in
        [("bump", released(LINEAR_BUMP)), ("guarded_bump", released(&guarded(LINEAR_BUMP)))]
    {
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        assert_eq!(live, 0, "{name}: blocks left live per 1000 cells");
    }
}

const COUNTING_LOOP: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn count(n, acc) {
    if n == 0 { acc } else {
        let xs = numbers(n)
        let l = len(xs)
        count(n - 1, acc + l)
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

/// A self tail call that makes a list on each turn: it must stay a jump, so
/// ten million turns run in constant stack natively, and each turn allocates
/// exactly its three cells.
#[test]
fn a_loop_that_makes_a_list_each_turn_stays_a_tail_call() {
    let scratch = Scratch::new("debug-heap-counting-loop");
    // Native binaries get a 64 MB stack on macOS and Windows: a frame per turn
    // still fits at a million turns, and overflows well before ten million.
    // The VM takes too long at this size, so the sum is checked directly.
    let path = scratch.write("count_long.flx", &COUNTING_LOOP.replace("SIZE", "10000000"));
    let file = path.to_string_lossy();
    let (native, stderr, code) = run(&scratch, &file, &["--native"], None);
    assert_eq!((native.as_str(), code), ("30000000\n", 0), "{stderr}");

    let at = |n: u64| {
        allocs_of(
            &scratch,
            &format!("count_{n}.flx"),
            &COUNTING_LOOP.replace("SIZE", &n.to_string()),
        )
    };
    let (small, large) = (at(1000), at(2000));
    assert_eq!(large.saturating_sub(small), 3000, "1000 turns: {small}, 2000 turns: {large}");
}

const LENDING_LOOP: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn walk(n, acc, prev) {
    if n == 0 { acc } else {
        walk(n - 1, acc + len(prev), numbers(n))
    }
}

fn main() {
    print(walk(SIZE, 0, []))
}
";

/// A self tail call that hands the list it made to the next turn, which only
/// reads it. Releasing it after the call would take a frame per turn and
/// overflow the stack at ten million turns. Instead `prev` is owned, since a
/// recursive tail call passes it a fresh value: the call gives the list away
/// and stays a jump, and the next turn releases it (KI-128).
#[test]
fn a_loop_that_lends_a_fresh_list_to_its_tail_call_stays_a_tail_call() {
    let scratch = Scratch::new("debug-heap-lending-loop");
    let path = scratch.write("lend_long.flx", &LENDING_LOOP.replace("SIZE", "10000000"));
    let file = path.to_string_lossy();
    let (native, stderr, code) = run(&scratch, &file, &["--native"], None);
    assert_eq!((native.as_str(), code), ("29999997\n", 0), "{stderr}");
}

const CLOSURE_PER_TURN: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn count(n, acc) {
    if n == 0 { acc } else {
        let xs = numbers(n)
        let f = \() -> len(xs)
        count(n - 1, acc + f())
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

const LAMBDA_READS_ITS_PARAMETER: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn count(n, acc, f) {
    if n == 0 { acc } else {
        count(n - 1, acc + f(numbers(n)), f)
    }
}

fn main() {
    print(count(SIZE, 0, \ys -> len(ys)))
}
";

const OWNED_THEN_READ: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn discard(xs, keep) {
    if keep { xs } else { [] }
}

fn keep_len(xs) {
    len(discard(xs, true))
}

fn count(n, acc) {
    if n == 0 { acc } else {
        let xs = numbers(n)
        let a = keep_len(xs)
        count(n - 1, acc + a + len(xs))
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

/// KI-121: a value whose last use reads it is released after that read: a
/// list only lent, a closure only called and the list it captured, a
/// lambda's parameter, and a list given away once and then read. Each turn
/// makes the same blocks, so none is left at 2000 turns that was not at
/// 1000.
#[test]
fn values_last_used_by_a_read_are_released() {
    let scratch = Scratch::new("debug-heap-last-read");
    let failures: Vec<String> = [
        ("counting_loop", COUNTING_LOOP),
        ("closure_per_turn", CLOSURE_PER_TURN),
        ("lambda_reads_its_parameter", LAMBDA_READS_ITS_PARAMETER),
        ("owned_then_read", OWNED_THEN_READ),
    ]
    .into_iter()
    .filter_map(|(name, program)| {
        let (_, _, live) = per_thousand_cells(&scratch, name, program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const LENT_TO_A_TAIL_CALL: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn bump(xs) {
    match xs {
        [h | t] -> [h + 1 | t],
        _ -> xs,
    }
}

fn total(xs) {
    match xs {
        [h | t] -> h + total(t),
        _ -> 0,
    }
}

fn turn(xs) {
    total(bump(xs))
}

fn count(n, acc) {
    if n == 0 { acc } else { count(n - 1, acc + turn(numbers(n))) }
}

fn main() {
    print(count(SIZE, 0))
}
";

const LOCAL_WALKER: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn walk(xs) {
    fn go(ys, n) {
        match ys {
            [h | t] -> go(t, n + h),
            _ -> n,
        }
    }
    go(xs, 0)
}

fn count(n, acc) {
    if n == 0 { acc } else { count(n - 1, acc + walk(numbers(n))) }
}

fn main() {
    print(count(SIZE, 0))
}
";

const LIST_COUNT: &str = r"import Flow.List as List

fn numbers(n) {
    [n, n + 1, n + 2]
}

fn count(n, acc) {
    if n == 0 { acc } else {
        count(n - 1, acc + List.count(numbers(n), \x -> x > 0))
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

const CAPTURING_LOCAL_CALLED_LAST: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn run(xs) {
    fn go(k, n) {
        if k == 0 { n + len(xs) } else { go(k - 1, n + 1) }
    }
    go(3, 0)
}

fn count(n, acc) {
    if n == 0 { acc } else { count(n - 1, acc + run(numbers(n))) }
}

fn main() {
    print(count(SIZE, 0))
}
";

/// KI-128: a value whose last use is a call in tail position, where the call
/// only reads it. The value was kept so the call stayed a tail call, and
/// nothing released it: the list `turn` rebuilds and lends to `total`, the
/// list a local function walks (and `List.count`, written the same way), a
/// local function called last with the list it captured, and the list the
/// lending loop hands to its next turn.
#[test]
fn a_value_lent_to_a_tail_call_is_released() {
    let scratch = Scratch::new("debug-heap-lent-to-tail-call");
    let failures: Vec<String> = [
        ("lent_to_a_tail_call", LENT_TO_A_TAIL_CALL),
        ("local_walker", LOCAL_WALKER),
        ("list_count", LIST_COUNT),
        ("capturing_local_called_last", CAPTURING_LOCAL_CALLED_LAST),
        ("lending_loop", LENDING_LOOP),
    ]
    .into_iter()
    .filter_map(|(name, program)| {
        let (_, _, live) = per_thousand_cells(&scratch, name, program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const OWNED_TEMPORARY: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn sum(xs) {
    match xs {
        [h | t] -> h + sum(t),
        _ -> 0,
    }
}

fn keep(xs) {
    xs
}

fn first(ys) {
    match ys {
        [h | _] -> h,
        _ -> 0,
    }
}

fn go(n) {
    let xs = numbers(n)
    let r = [len(keep(xs))]
    first(r) + sum(xs)
}

fn count(n, acc) {
    if n == 0 { acc } else { count(n - 1, acc + go(n)) }
}

fn main() {
    print(count(SIZE, 0))
}
";

const PRINTED_TEMPORARY: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn sum(xs) {
    match xs {
        [h | t] -> h + sum(t),
        _ -> 0,
    }
}

fn keep(xs) {
    xs
}

fn go(n) with IO {
    let xs = numbers(n)
    print(len(keep(xs)))
    sum(xs)
}

fn count(n, acc) with IO {
    if n == 0 { acc } else { count(n - 1, acc + go(n)) }
}

fn main() with IO {
    print(count(SIZE, 0))
}
";

const BUILT_STRING: &str = r#"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn sum(xs) {
    match xs {
        [h | t] -> h + sum(t),
        _ -> 0,
    }
}

fn go(n) {
    let xs = numbers(n)
    let name = "n" + to_string(len(xs))
    let pair = (len(xs), name)
    pair.0 + sum(xs) + len(name)
}

fn count(n, acc) {
    if n == 0 { acc } else { count(n - 1, acc + go(n)) }
}

fn main() {
    print(count(SIZE, 0))
}
"#;

/// KI-129: `len(keep(xs))` is computed in a block that copies its result
/// before releasing what `keep` returned. Given straight to an owned use, a
/// list cell or a `print`, that result was taken for a view of `xs`, and
/// `xs` was never released. A string built with `+` from `xs` was taken for
/// one too, and was never released itself.
#[test]
fn a_value_copied_into_a_temporary_given_away_is_released() {
    let scratch = Scratch::new("debug-heap-owned-temporary");
    let failures: Vec<String> = [
        ("owned_temporary", OWNED_TEMPORARY),
        ("printed_temporary", PRINTED_TEMPORARY),
        ("built_string", BUILT_STRING),
    ]
    .into_iter()
    .filter_map(|(name, program)| {
        let (_, _, live) = per_thousand_cells(&scratch, name, program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
