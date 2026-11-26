// Port of benchmarks/flux/tree_explore_direct.flx: Rust has no effect
// handlers, so the state is threaded by hand through the branches and each
// choice expands into both branches' results. Subtrees are shared, so they
// are counted references.
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

fn operator(x: i64, y: i64) -> i64 {
    (x - 503 * y + 37).abs() % 1009
}

fn explore(t: &Tree, s: i64) -> (Vec<i64>, i64) {
    match t {
        Tree::Leaf => (vec![s], s),
        Tree::Node(l, v, r) => {
            let (xs, s2) = explore(l, operator(s, *v));
            let (ys, s4) = explore(r, operator(s2, *v));
            let mut out: Vec<i64> = xs.into_iter().map(|x| operator(*v, x)).collect();
            out.extend(ys.into_iter().map(|y| operator(*v, y)));
            (out, s4)
        }
    }
}

fn main() {
    let n = env::args().nth(1).and_then(|value| value.parse::<i64>().ok()).unwrap_or(16);
    let tree = make(n);
    let mut state = 0;
    for _ in 0..10 {
        let (xs, _) = explore(&tree, state);
        state = xs.into_iter().fold(0, i64::max);
    }
    println!("{}", state);
}
