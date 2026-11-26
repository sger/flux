// Port of benchmarks/flux/generator_direct.flx: Rust has no effect
// handlers, so the tree is summed by a walk in order. Subtrees are shared,
// so they are counted references.
use std::env;
use std::rc::Rc;

enum Tree {
    Leaf,
    Node(Rc<Tree>, i64, Rc<Tree>),
}

fn make(n: i64) -> Rc<Tree> {
    if n == 0 {
        Rc::new(Tree::Leaf)
    } else {
        let t = make(n - 1);
        Rc::new(Tree::Node(t.clone(), n, t))
    }
}

fn total(t: &Tree, a: i64) -> i64 {
    match t {
        Tree::Leaf => a,
        Tree::Node(l, v, r) => total(r, total(l, a) + v),
    }
}

fn main() {
    let n = env::args().nth(1).and_then(|value| value.parse::<i64>().ok()).unwrap_or(25);
    println!("{}", total(&make(n), 0));
}
