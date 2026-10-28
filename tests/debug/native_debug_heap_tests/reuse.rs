use super::*;

const FORWARDED_CHILD: &str = r"type Pair2<T> = Pair2(T, T)

fn numbers(n) {
    [n, n + 1, n + 2]
}

fn forwarded_child(xs, acc) {
    match xs {
        [y | ys] -> Pair2([y | acc], ys),
        _ -> Pair2(xs, acc),
    }
}

fn lengths(p) {
    match p {
        Pair2(a, b) -> len(a) + len(b),
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let p = forwarded_child(numbers(n), [n])
        count(n - 1, total + lengths(p))
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

const GROW_OTHER_SHAPE: &str = r"type Shape = Circle(Int) | Square(Int)

fn make(n) {
    if n % 2 == 0 { Circle(n) } else { Square(n) }
}

fn grow(s) {
    match s {
        Circle(r) -> Circle(r + 1),
        _ -> Circle(0),
    }
}

fn size(s) {
    match s {
        Circle(r) -> r,
        Square(w) -> w,
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let s = grow(make(n))
        count(n - 1, total + size(s))
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

const WALK_GIVES_ITS_LIST_AWAY: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn keep(token, v) {
    if len(token) < 0 { match [token] { _ -> v } } else { v }
}

fn walk(xs) {
    match xs {
        [h | t] -> [keep(xs, h) | walk(t)],
        _ -> [],
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let ys = walk(numbers(n))
        count(n - 1, total + len(ys))
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

/// KI-121: a value given away or reused on one path is released on the
/// others: the arm that does not rebuild a circle releases the square it
/// matched, and the arm that gives the list a copy releases the list after
/// its last read.
#[test]
fn a_value_moved_on_one_path_is_released_on_the_others() {
    let scratch = Scratch::new("debug-heap-moved-on-one-path");
    let failures: Vec<String> = [
        ("grow_other_shape", GROW_OTHER_SHAPE),
        ("walk_gives_its_list_away", WALK_GIVES_ITS_LIST_AWAY),
    ]
    .into_iter()
    .filter_map(|(name, program)| {
        let (_, _, live) = per_thousand_cells(&scratch, name, program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const LENS_OF_ROWS: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn rows(n) {
    [numbers(n), numbers(n + 1)]
}

fn lens(xss) {
    match xss {
        [xs | rest] -> [len(xs) | lens(rest)],
        _ -> [],
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let ys = lens(rows(n))
        count(n - 1, total + len(ys))
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

/// KI-121: a field the arm copies before releasing its cell, and then only
/// reads, is released after that read. Each row is read once and left
/// behind, while the rebuild still reuses the outer list's cells: 8 blocks
/// a turn, all freed.
#[test]
fn a_field_only_read_after_its_cell_is_released_is_released_too() {
    let scratch = Scratch::new("debug-heap-field-only-read");
    let (allocs, _, live) = per_thousand_cells(&scratch, "lens_of_rows", LENS_OF_ROWS);
    assert_eq!((allocs, live), (8000, 0), "(allocs, live) per 1000 turns");
}

const ARRAY_TO_LIST: &str = r"fn to_list(arr, idx) {
    if idx >= len(arr) { [] } else {
        match arr[idx] {
            Some(v) -> [v | to_list(arr, idx + 1)],
            _ -> to_list(arr, idx + 1)
        }
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let ys = to_list([|n, n + 1, n + 2|], 0)
        count(n - 1, total + len(ys))
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

/// KI-121: the option an index returns is matched and its value moved into
/// a new cell; nothing reuses the option, so it is released, whether the
/// cell is built before or after that release.
#[test]
fn an_option_matched_and_not_reused_is_released() {
    let scratch = Scratch::new("debug-heap-array-to-list");
    let (allocs, _, live) = per_thousand_cells(&scratch, "array_to_list", ARRAY_TO_LIST);
    assert_eq!(live, 0, "live per 1000 turns ({allocs} allocs)");
}

const BRANCH_REBUILD: &str = r"fn cell(n) { Some(None) }
fn read(opt) { match opt { Some(_) -> 1, _ -> 0 } }

fn rebuild(opt, keep) {
    match opt {
        Some(v) -> if keep { Some(v) } else { None },
        _ -> None
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let xs = cell(n)
        let ys = rebuild(xs, n % 2 == 0)
        let next = total + read(ys) + EXTRA_READ
        count(n - 1, next)
    }
}

fn main() { print(count(SIZE, 0)) }
";

// A nullary payload isolates shell accounting from extra copies of heap fields.
// The later read retains a second reference to the same cell across rebuild.
#[test]
fn a_matched_cell_reused_on_only_one_path_is_released() {
    let scratch = Scratch::new("debug-heap-branch-rebuild");
    for (name, extra_read) in [("unique", "0"), ("shared", "read(xs)")] {
        let program = BRANCH_REBUILD.replace("EXTRA_READ", extra_read);
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        assert_eq!(live, 0, "{name}: live per 1000 turns");
    }
}

const MERGE: &str = r"fn merge(left, right) {
    match left {
        [a | at] -> match right {
            [b | bt] -> if a <= b { [a | merge(at, right)] } else { [b | merge(left, bt)] },
            _ -> left
        },
        _ -> right
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let ys = merge([n, n + 2, n + 4], [n + 1, n + 3, n + 5])
        count(n - 1, total + len(ys))
    }
}

fn main() { print(count(SIZE, 0)) }
";

/// KI-121: each arm keeps the other list whole and rebuilds in its own cell.
/// The tail it passes on is copied once, before that cell may be released;
/// a second copy where it is passed is a reference nothing gives back.
#[test]
fn a_merge_that_rebuilds_in_its_own_cells_copies_each_field_once() {
    let scratch = Scratch::new("debug-heap-merge");
    let (allocs, _, live) = per_thousand_cells(&scratch, "merge", MERGE);
    assert_eq!(live, 0, "live per 1000 turns ({allocs} allocs)");
}

/// A cons rebuilt in the cell of the list it was matched from, inside a
/// fresh wrapper, is built once. A second, unused copy of it was left
/// behind each turn, holding its fields with references it never had.
#[test]
fn a_forwarded_child_leaves_no_cell_behind() {
    let scratch = Scratch::new("debug-heap-forwarded-child");
    let (_, _, live) = per_thousand_cells(&scratch, "forwarded_child", FORWARDED_CHILD);
    assert_eq!(live, 0, "blocks left live per 1000 turns");
}

const ZEROS_OF_ROWS: &str = r"fn rows(n) {
    [[n], [n + 1], [n + 2]]
}

fn zeros(xss) {
    match xss {
        [_ | rest] -> [0 | zeros(rest)],
        _ -> []
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let ys = zeros(rows(n))
        count(n - 1, total + len(ys))
    }
}

fn main() { print(count(SIZE, 0)) }
";

const KEEP_LEFT: &str = r"type Pair2<T> = Pair2(T, T)

fn make(n) { Pair2([n], [n + 1]) }

fn keep_left(p) {
    match p {
        Pair2(a, _) -> Pair2(a, []),
    }
}

fn size(p) { match p { Pair2(a, b) -> len(a) + len(b) } }

fn count(n, total) {
    if n == 0 { total } else {
        let p = keep_left(make(n))
        count(n - 1, total + size(p))
    }
}

fn main() { print(count(SIZE, 0)) }
";

const RESET_OPTION: &str = r"fn make(n) { Some([n]) }

fn reset(o) {
    match o {
        Some(_) -> Some([0]),
        _ -> None
    }
}

fn size(o) { match o { Some(xs) -> len(xs), _ -> 0 } }

fn count(n, total) {
    if n == 0 { total } else {
        let o = reset(make(n))
        count(n - 1, total + size(o))
    }
}

fn main() { print(count(SIZE, 0)) }
";

const SUM_PAIRS: &str = r"fn numbers(n) {
    [n, n + 1, n + 2, n + 3]
}

fn pairs(xs) {
    match xs {
        [a | [b | t]] -> [a + b | pairs(t)],
        _ -> []
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let ys = pairs(numbers(n))
        count(n - 1, total + len(ys))
    }
}

fn main() { print(count(SIZE, 0)) }
";

/// A cell rebuilt in place holds new fields, so each old field nothing took
/// over is released first: a wildcard, an unused binder, an inner cell. The
/// first three split into unique and shared paths; the last two reuse the
/// cell where its plain release was.
#[test]
fn a_reused_cell_releases_the_fields_it_does_not_keep() {
    let scratch = Scratch::new("debug-heap-reused-cell-fields");
    let failures: Vec<String> = [
        ("wildcard", ZEROS_OF_ROWS.to_string()),
        ("unused", ZEROS_OF_ROWS.replace("[_ | rest]", "[xs | rest]")),
        ("constructor", KEEP_LEFT.to_string()),
        ("plain", RESET_OPTION.to_string()),
        ("inner_cell", SUM_PAIRS.to_string()),
    ]
    .into_iter()
    .filter_map(|(name, program)| {
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const RETURN_A_COUNT: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn pick(n) {
    let xs = numbers(n)
    let ys = numbers(n + 1)
    if n > 0 { return len(xs) }
    len(ys)
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + pick(n)) }
}

fn main() { print(count(SIZE, 0)) }
";

const RETURN_A_LIST: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn pick(n) {
    let xs = numbers(n)
    let ys = numbers(n + 1)
    if n > 0 { return xs }
    ys
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + len(pick(n))) }
}

fn main() { print(count(SIZE, 0)) }
";

const RETURN_PAST_A_PARAMETER: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn pick(xs, n) {
    if n > 0 { return [n] }
    xs
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + len(pick(numbers(n), n))) }
}

fn main() { print(count(SIZE, 0)) }
";

const RETURN_A_FIELD: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn pick(n) {
    let rows = [numbers(n), numbers(n + 1)]
    match rows {
        [r | _] -> if len(r) > 0 { return r },
        _ -> if n < 0 { return numbers(0) }
    }
    numbers(0)
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + len(pick(n))) }
}

fn main() { print(count(SIZE, 0)) }
";

/// An early return leaves the function with the values it holds still to
/// release: those it never gives away go before it does, and a field it
/// returns is copied out of its list first.
#[test]
fn an_early_return_releases_what_the_function_holds() {
    let scratch = Scratch::new("debug-heap-early-return");
    let failures: Vec<String> = [
        ("count", RETURN_A_COUNT),
        ("list", RETURN_A_LIST),
        ("parameter", RETURN_PAST_A_PARAMETER),
        ("field", RETURN_A_FIELD),
    ]
    .into_iter()
    .filter_map(|(name, program)| {
        let (_, _, live) = per_thousand_cells(&scratch, name, program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const LEND_A_FUNCTION: &str = r"fn double(x) { x * 2 }

fn apply(f, x) { f(x) }

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + apply(double, n)) }
}

fn main() { print(count(SIZE, 0)) }
";

const LEND_A_LITERAL: &str = r#"fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + len("hello")) }
}

fn main() { print(count(SIZE, 0)) }
"#;

const RETURN_A_LITERAL: &str = r#"fn tag(n) {
    if n % 2 == 0 { "even" } else { "odd" }
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + len(tag(n))) }
}

fn main() { print(count(SIZE, 0)) }
"#;

/// A top-level function used as a value and a string literal are the same
/// value every time, so each is made once, outside the heap, and never
/// released: no use allocates, and a use that only lends one leaves nothing
/// behind.
#[test]
fn a_function_or_literal_used_as_a_value_is_never_allocated() {
    let scratch = Scratch::new("debug-heap-static-values");
    let failures: Vec<String> = [
        ("lent_function", LEND_A_FUNCTION),
        ("lent_literal", LEND_A_LITERAL),
        ("returned_literal", RETURN_A_LITERAL),
    ]
    .into_iter()
    .filter_map(|(name, program)| {
        let (allocs, _, live) = per_thousand_cells(&scratch, name, program);
        (allocs != 0 || live != 0).then(|| {
            format!("{name}: {allocs} blocks allocated and {live} left live per 1000 turns")
        })
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
