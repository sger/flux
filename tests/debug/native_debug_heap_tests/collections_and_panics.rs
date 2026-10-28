use super::*;

const LOCAL_FUNCTION_PER_TURN: &str = r"fn turn(n) {
    LOCAL
}

fn loop(n, total) {
    if n == 0 { total } else { loop(n - 1, total + turn(n)) }
}

fn main() with IO {
    print(to_string(loop(SIZE, 0)))
}
";

/// A local recursive function calls itself directly, so it builds no
/// closure of itself on entry: one per entry was left behind. The same for
/// local functions that call each other, and for one that captures.
#[test]
fn a_local_function_leaves_no_closure_behind() {
    let scratch = Scratch::new("debug-heap-local-function");
    let failures: Vec<String> = [
        (
            "own",
            "fn go(i, acc) {\n        if i == 0 { acc } else { go(i - 1, acc + i) }\n    }\n    let r = go(3, n)\n    r + 0",
        ),
        (
            "captures",
            "let k = n % 2\n    fn go(i, acc) {\n        if i == 0 { acc } else { go(i - 1, acc + i + k) }\n    }\n    let r = go(3, n)\n    r + 0",
        ),
        (
            "group",
            "fn even(i) {\n        if i == 0 { 1 } else { odd(i - 1) }\n    }\n    fn odd(i) {\n        if i == 0 { 0 } else { even(i - 1) }\n    }\n    let r = even(n % 4)\n    r + 0",
        ),
    ]
    .into_iter()
    .filter_map(|(name, local)| {
        let program = LOCAL_FUNCTION_PER_TURN.replace("LOCAL", local);
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const WIDE_ARRAY: &str = r"import Flow.Array exposing (concat)

fn build(lo, hi) {
    if hi - lo == 1 { [| [lo] |] } else {
        let mid = (lo + hi) / 2
        concat(build(lo, mid), build(mid, hi))
    }
}

fn main() with IO {
    print(len(build(0, SIZE)))
}
";

/// An array releases every element, each a list of its own. Its header
/// counted the elements to release in one byte, capped at 255, so the rest
/// stayed live; 70000 is past the 65535 a 16-bit index reaches too (KI-123).
/// Halving builds it without the quadratic copying of a `push` per element.
#[test]
fn an_array_releases_every_element() {
    let scratch = Scratch::new("debug-heap-wide-array");
    let at = |n: u64| {
        stats_of(
            &scratch,
            &format!("wide_array_{n}.flx"),
            &WIDE_ARRAY.replace("SIZE", &n.to_string()),
        )
        .2
    };
    let (one, wide) = (at(1), at(70000));
    assert_eq!(wide, one, "an array of 70000 lists left {wide} blocks live, one of 1 left {one}");
}

const MAP_PER_TURN: &str = r"import Flow.Map exposing (set, delete, merge, size)

fn size_of(m) { size(m) }

fn wide(i, n, m) {
    if i == 0 { m } else { wide(i - 1, n, set(m, i, [n + i])) }
}

fn turn(n) {
    TURN
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + turn(n)) }
}

fn main() with IO {
    print(to_string(count(SIZE, 0)))
}
";

/// A map releases its nodes, keys and values, and maps made from one
/// another release what they share once both are gone. A release freed
/// nothing under the root, which was all that kept the uncounted sharing
/// between maps safe; a map of 100 keys has branches inside branches
/// (KI-124).
#[test]
fn a_map_releases_its_nodes_keys_and_values() {
    let scratch = Scratch::new("debug-heap-map");
    let failures: Vec<String> = [
        ("built", r#"size_of(set(set({}, "a", [n]), "b", [n + 1]))"#),
        ("literal", r#"size_of({"a": [n], "b": [n + 1]})"#),
        (
            "shared",
            r#"let m1 = set({}, "a", [n])
    let m2 = set(m1, "c", [n])
    size_of(m1) + size_of(m2)"#,
        ),
        ("deleted", r#"size_of(delete(set(set({}, "a", [n]), "b", [n]), "a"))"#),
        ("merged", r#"size_of(merge(set({}, "a", [n]), set({}, "b", [n])))"#),
        ("wide", "size_of(wide(100, n, {}))"),
    ]
    .into_iter()
    .filter_map(|(name, body)| {
        let program = MAP_PER_TURN.replace("TURN", body);
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const ALIAS_PER_TURN: &str = r"import Flow.Array as Array
import Flow.List as List
import Flow.Map as Map

fn turn(n) {
    TURN
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + turn(n)) }
}

fn main() with IO {
    print(to_string(count(SIZE, 0)))
}
";

/// A call through a module alias releases what it was lent. The callee was
/// hoisted into a temporary, so Aether gave the arguments away to an
/// unknown function, and a module function that only borrows them, such as
/// `Map.set`, never released them (KI-130). `List.reverse` takes its
/// argument, so it was right before.
#[test]
fn a_call_through_a_module_alias_releases_its_arguments() {
    let scratch = Scratch::new("debug-heap-alias");
    let failures: Vec<String> = [
        ("array_push", "len(Array.push([| [n] |], [n + 1]))"),
        ("map_set", r#"Map.size(Map.set(Map.set({}, "a", [n]), "b", [n + 1]))"#),
        (
            "map_get",
            r#"let m = Map.set({}, "a", [n])
    match Map.get(m, "a") { Some(xs) -> len(xs), _ -> 0 }"#,
        ),
        ("list_reverse", "len(List.reverse([n, n + 1]))"),
    ]
    .into_iter()
    .filter_map(|(name, body)| {
        let program = ALIAS_PER_TURN.replace("TURN", body);
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const FAILED_FS_CALLS: &str = r#"import Flow.Fs as Fs
import Flow.IoError as Io

fn kind(r) with IO {
    match r {
        Ok(_) -> println("ok"),
        Err(e) -> println(Io.kind_name(Io.error_kind(e))),
    }
}

fn main() with IO {
    let base = "target/test-scratch/debug_heap_failed_fs"
    kind(Fs.remove_file(base + "/absent.txt"))
    kind(Fs.rename(base + "/absent", base + "/t"))
    kind(Fs.write_file(base + "/no/parent/f.txt", "x"))
    kind(Fs.remove_dir_all(base + "/absent"))
    kind(Fs.read_file(base + "/absent.txt"))
}
"#;

/// A failed file operation's error holds the path it was lent: the error
/// took it without counting it, so the caller's release and the error's
/// freed it twice (KI-131). `read_file` builds its error apart from the
/// others.
#[test]
fn a_failed_file_operation_counts_the_path_its_error_holds() {
    let scratch = Scratch::new("debug-heap-failed-fs");
    let (allocs, frees, live) = stats_of(&scratch, "failed_fs.flx", FAILED_FS_CALLS);
    assert_eq!(
        live, 1,
        "{allocs} allocated, {frees} freed: only the block every program that prints leaves should be live (KI-119)"
    );
}

const PANIC_PER_TURN: &str = r#"import Flow.Assert exposing (try_catch)

effect Ask {
    ask : (Int) -> Int
}

effect Tell {
    tell : (Int) -> Int
}

fn numbers(n) {
    [n, n + 1, n + 2]
}

fn explode(n) -> Int {
    if n >= 0 { panic("boom") } else { n }
}

fn boom(n) -> Int {
    let xs = numbers(n)
    let r = explode(n)
    r + len(xs)
}

fn outer(n) -> Int {
    let ys = numbers(n)
    let r = boom(n)
    r + len(ys)
}

fn before_perform(n) -> Int with Ask {
    explode(n) + perform Ask.ask(n)
}

fn after_resume(n) -> Int with Ask {
    perform Ask.ask(n) + explode(n)
}

fn under_two(n) -> Int with Ask, Tell {
    perform Tell.tell(n) + perform Ask.ask(n) + explode(n)
}

fn attempt(n) -> Int {
    let caught = try_catch(\() -> BODY)
    1
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + attempt(n)) }
}

fn main() with IO {
    print(to_string(count(SIZE, 0)))
}
"#;

/// A panic caught further up releases what the frames between held. It
/// jumped straight to the catch point, past every release still to come in
/// those frames, so `boom`'s list stayed live (KI-126). What a skipped
/// `handle` builds is the effect runtime's, not a frame's (KI-133).
#[test]
#[cfg_attr(windows, ignore = "Windows catches a panic with SEH, which runs no landing pads")]
fn a_caught_panic_releases_what_the_frames_it_skipped_held() {
    let scratch = Scratch::new("debug-heap-caught-panic");
    let failures: Vec<String> = [("held_list", "boom(n)"), ("two_frames", "outer(n)")]
        .into_iter()
        .filter_map(|(name, body)| {
            let program = PANIC_PER_TURN.replace("BODY", body);
            let (_, _, live) = per_thousand_cells(&scratch, name, &program);
            (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A panic caught further up releases what a `handle` it skipped built:
/// the evidence the handle installed, which only its prompt released, and
/// the prompt's own references to the clause's `resume` and argument
/// (KI-133).
#[test]
#[cfg_attr(windows, ignore = "Windows catches a panic with SEH, which runs no landing pads")]
fn a_caught_panic_releases_what_a_skipped_handle_built() {
    let scratch = Scratch::new("debug-heap-skipped-handle");
    let failures: Vec<String> = [
        (
            "in_body",
            "before_perform(n) handle Ask { ask(resume, x) -> resume(x + 1) }",
        ),
        (
            "after_resume",
            "after_resume(n) handle Ask { ask(resume, x) -> resume(x + 1) }",
        ),
        (
            "nested",
            "(under_two(n) handle Ask { ask(resume, x) -> resume(x + 1) }) handle Tell { tell(resume, x) -> resume(x) }",
        ),
    ]
    .into_iter()
    .filter_map(|(name, body)| {
        let program = PANIC_PER_TURN.replace("BODY", body);
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
