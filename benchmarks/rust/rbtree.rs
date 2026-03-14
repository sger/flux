#[derive(Clone, Copy)]
enum Color {
    Red,
    Black,
}

#[derive(Clone)]
enum Tree {
    Leaf,
    Node(Color, Box<Tree>, i64, bool, Box<Tree>),
}

const LIMIT: i64 = 100_000;

fn count_true(tree: &Tree) -> i64 {
    match tree {
        Tree::Leaf => 0,
        Tree::Node(_, left, _, value, right) => {
            count_true(left) + i64::from(*value) + count_true(right)
        }
    }
}

fn is_red(tree: &Tree) -> bool {
    matches!(tree, Tree::Node(Color::Red, _, _, _, _))
}

fn balance1(left_tree: Tree, right_tree: Tree) -> Tree {
    match left_tree {
        Tree::Node(_, _, kv, vv, tail) => match right_tree {
            Tree::Node(_, left, ky, vy, right) => match (*left, *right) {
                (Tree::Node(Color::Red, l, kx, vx, r1), r2) => Tree::Node(
                    Color::Red,
                    Box::new(Tree::Node(Color::Black, l, kx, vx, r1)),
                    ky,
                    vy,
                    Box::new(Tree::Node(Color::Black, Box::new(r2), kv, vv, tail)),
                ),
                (l1, Tree::Node(Color::Red, l2, kx, vx, r)) => Tree::Node(
                    Color::Red,
                    Box::new(Tree::Node(Color::Black, Box::new(l1), ky, vy, l2)),
                    kx,
                    vx,
                    Box::new(Tree::Node(Color::Black, r, kv, vv, tail)),
                ),
                (l, r) => Tree::Node(
                    Color::Black,
                    Box::new(Tree::Node(Color::Red, Box::new(l), ky, vy, Box::new(r))),
                    kv,
                    vv,
                    tail,
                ),
            },
            _ => Tree::Leaf,
        },
        _ => Tree::Leaf,
    }
}

fn balance2(left_tree: Tree, right_tree: Tree) -> Tree {
    match left_tree {
        Tree::Node(_, tree0, kv, vv, _) => match right_tree {
            Tree::Node(_, left, ky, vy, right) => match (*left, *right) {
                (Tree::Node(Color::Red, l, kx1, vx1, r1), r2) => Tree::Node(
                    Color::Red,
                    Box::new(Tree::Node(Color::Black, tree0, kv, vv, l)),
                    kx1,
                    vx1,
                    Box::new(Tree::Node(Color::Black, r1, ky, vy, Box::new(r2))),
                ),
                (l1, Tree::Node(Color::Red, l2, kx2, vx2, r2)) => Tree::Node(
                    Color::Red,
                    Box::new(Tree::Node(Color::Black, tree0, kv, vv, Box::new(l1))),
                    ky,
                    vy,
                    Box::new(Tree::Node(Color::Black, l2, kx2, vx2, r2)),
                ),
                (l, r) => Tree::Node(
                    Color::Black,
                    tree0,
                    kv,
                    vv,
                    Box::new(Tree::Node(Color::Red, Box::new(l), ky, vy, Box::new(r))),
                ),
            },
            _ => Tree::Leaf,
        },
        _ => Tree::Leaf,
    }
}

fn ins(tree: Tree, kx: i64, vx: bool) -> Tree {
    match tree {
        Tree::Leaf => Tree::Node(
            Color::Red,
            Box::new(Tree::Leaf),
            kx,
            vx,
            Box::new(Tree::Leaf),
        ),
        Tree::Node(Color::Red, a, ky, vy, b) => {
            if kx < ky {
                Tree::Node(Color::Red, Box::new(ins(*a, kx, vx)), ky, vy, b)
            } else if ky < kx {
                Tree::Node(Color::Red, a, ky, vy, Box::new(ins(*b, kx, vx)))
            } else {
                Tree::Node(Color::Red, a, kx, vx, b)
            }
        }
        Tree::Node(Color::Black, a, ky, vy, b) => {
            if kx < ky {
                if is_red(&a) {
                    balance1(
                        Tree::Node(Color::Black, Box::new(Tree::Leaf), ky, vy, b),
                        ins(*a, kx, vx),
                    )
                } else {
                    Tree::Node(Color::Black, Box::new(ins(*a, kx, vx)), ky, vy, b)
                }
            } else if ky < kx {
                if is_red(&b) {
                    balance2(
                        Tree::Node(Color::Black, a, ky, vy, Box::new(Tree::Leaf)),
                        ins(*b, kx, vx),
                    )
                } else {
                    Tree::Node(Color::Black, a, ky, vy, Box::new(ins(*b, kx, vx)))
                }
            } else {
                Tree::Node(Color::Black, a, kx, vx, b)
            }
        }
    }
}

fn set_black(tree: Tree) -> Tree {
    match tree {
        Tree::Node(_, left, key, value, right) => Tree::Node(Color::Black, left, key, value, right),
        other => other,
    }
}

fn insert(tree: Tree, key: i64, value: bool) -> Tree {
    if is_red(&tree) {
        set_black(ins(tree, key, value))
    } else {
        ins(tree, key, value)
    }
}

fn mk_map_aux(n: i64, tree: Tree) -> Tree {
    if n == 0 {
        tree
    } else {
        let n1 = n - 1;
        let next = insert(tree, n1, n1 % 10 == 0);
        mk_map_aux(n1, next)
    }
}

fn main() {
    let handle = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            println!("{}", count_true(&mk_map_aux(LIMIT, Tree::Leaf)));
        })
        .expect("spawn rbtree benchmark");
    handle.join().expect("run rbtree benchmark");
}
