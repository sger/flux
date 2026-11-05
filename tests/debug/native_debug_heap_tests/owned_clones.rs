use super::*;

/// Each shape of `examples/aether/owned_clones.flx`, with the blocks it took before
/// consuming clones existed (proposal 0200 C0). A clone frees what its caller would
/// have, so it changes when a block goes and never how many there are.
const SHAPES: &[(&str, &str, (u64, u64, u64))] = &[
    ("sum_trees", "print(sum_trees(3, 3, 0))", (27, 26, 1)),
    ("dead_list", "print(list_size(count_up(5, [])))", (11, 10, 1)),
    (
        "kept_in_list",
        "let listed = make_tree(2)
    let held = [listed]
    print(check_tree(listed))
    print(first_tree_size(held))",
        (11, 10, 1),
    ),
    ("even_odd", "print(even_walk(make_tree(2)))", (9, 8, 1)),
    ("two_params", "print(bit(same(make_tree(2), make_tree(2))))", (12, 11, 1)),
    (
        "kept_lookup",
        "let keyed = insert(insert(insert(KLeaf, 5), 3), 7)
    print(bit(has(keyed, 3)))
    print(keyed_size(keyed))",
        (10, 9, 1),
    ),
    (
        "effectful",
        "let noted = noted_walk(make_tree(1)) handle Note { note(resume) -> resume(1) }
    print(noted)",
        (17, 16, 1),
    ),
    ("field_handed_on", "print(nodes(make_tree(2)))", (10, 9, 1)),
    (
        "same_twice",
        "let twice = make_tree(1)
    print(bit(same(twice, twice)))
    print(check_tree(twice))",
        (8, 7, 1),
    ),
    (
        "closure",
        "let closed = make_tree(1)
    let read = \\() -> check_tree(closed)
    print(check_tree(closed))
    print(read())",
        (9, 8, 1),
    ),
    (
        "continuation",
        "let resumed = read_either_way(make_tree(1)) handle Flip { flip(resume) -> resume(true) + resume(false) }
    print(resumed)",
        (12, 11, 1),
    ),
    (
        "tasks",
        "let shared = make_tree(3)
    let one = spawn(\\() -> check_tree(shared))
    let two = spawn(\\() -> check_tree(shared))
    print(blocking_join(one))
    print(blocking_join(two))
    print(check_tree(shared))",
        (19, 18, 1),
    ),
];

#[test]
fn a_consuming_clone_frees_exactly_what_its_caller_would_have() {
    let fixture = include_str!("../../../examples/aether/owned_clones.flx");
    let definitions = &fixture[..fixture.find("fn main() with IO").expect("the fixture's main")];
    let scratch = Scratch::new("debug-heap-owned-clones");
    for (name, body, expected) in SHAPES {
        let program = format!("{definitions}fn main() with IO {{\n    {body}\n}}\n");
        assert_eq!(stats_of(&scratch, &format!("{name}.flx"), &program), *expected, "{name}");
    }
}

/// A clone's call to itself in tail position stays a jump, so a walk over a list
/// this long needs no stack. A call would overflow it.
#[test]
fn a_tail_call_in_a_clone_stays_a_jump() {
    let scratch = Scratch::new("debug-heap-owned-clone-tail");
    let (_, _, live) = stats_of(
        &scratch,
        "tail.flx",
        "fn build(n: Int, acc: List<Int>) -> List<Int> {
    if n == 0 { acc } else { build(n - 1, [n | acc]) }
}

fn walk(xs: List<Int>, acc: Int) -> Int {
    match xs {
        [_ | rest] -> walk(rest, acc + 1),
        _ -> acc,
    }
}

fn main() {
    print(walk(build(2000000, []), 0))
}
",
    );
    assert_eq!(live, 1);
}

/// An unboxed field is a copy of what it read, so reading it after the clone has
/// freed the cell is safe, and the call needn't stay on the original for it.
#[test]
fn an_unboxed_field_read_later_still_goes_to_the_clone() {
    let scratch = Scratch::new("debug-heap-owned-clone-unboxed");
    let program = "type Keyed = KLeaf | KNode(Keyed, Int, Keyed)

fn keyed_size(tree: Keyed) -> Int {
    match tree {
        KLeaf -> 0,
        KNode(left, _, right) -> keyed_size(left) + keyed_size(right) + 1,
    }
}

fn make_keyed(n: Int) -> Keyed {
    if n > 0 { KNode(make_keyed(n - 1), n, KLeaf) } else { KLeaf }
}

fn size_and_key(n: Int) -> Int {
    let tree = make_keyed(n)
    match tree {
        KNode(_, at, _) -> keyed_size(tree) + at,
        KLeaf -> 0,
    }
}

fn main() {
    print(size_and_key(4))
}
";
    assert_eq!(stats_of(&scratch, "unboxed.flx", program), (6, 5, 1));
    let path = scratch.write("plan.flx", program);
    let (plan, _, code) =
        run(&scratch, &path.to_string_lossy(), &["--native", "--dump-aether"], None);
    assert_eq!(code, 0, "{plan}");
    assert!(plan.contains("aether_call[owned] keyed_size$own"), "{plan}");
}
