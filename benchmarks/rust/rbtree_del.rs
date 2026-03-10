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

#[derive(Clone)]
struct Del(Tree, bool);

#[derive(Clone)]
struct Delmin(Del, i64, bool);

const LIMIT: i64 = 100_000;

fn count_true(tree: &Tree) -> i64 {
    match tree {
        Tree::Leaf => 0,
        Tree::Node(_, left, _, value, right) => count_true(left) + i64::from(*value) + count_true(right),
    }
}

fn is_red(tree: &Tree) -> bool {
    matches!(tree, Tree::Node(Color::Red, _, _, _, _))
}

fn is_black_color(color: Color) -> bool {
    matches!(color, Color::Black)
}

fn balance_left(l: Tree, k: i64, v: bool, r: Tree) -> Tree {
    match l {
        Tree::Leaf => Tree::Leaf,
        Tree::Node(_, left, ky, vy, ry) => match (*left, *ry) {
            (Tree::Node(Color::Red, lx, kx, vx, rx), ryv) => Tree::Node(
                Color::Red,
                Box::new(Tree::Node(Color::Black, lx, kx, vx, rx)),
                ky,
                vy,
                Box::new(Tree::Node(Color::Black, Box::new(ryv), k, v, Box::new(r))),
            ),
            (ly, Tree::Node(Color::Red, lx, kx, vx, rx)) => Tree::Node(
                Color::Red,
                Box::new(Tree::Node(Color::Black, Box::new(ly), ky, vy, lx)),
                kx,
                vx,
                Box::new(Tree::Node(Color::Black, rx, k, v, Box::new(r))),
            ),
            (lx, rx) => Tree::Node(
                Color::Black,
                Box::new(Tree::Node(Color::Red, Box::new(lx), ky, vy, Box::new(rx))),
                k,
                v,
                Box::new(r),
            ),
        },
    }
}

fn balance_right(l: Tree, k: i64, v: bool, r: Tree) -> Tree {
    match r {
        Tree::Leaf => Tree::Leaf,
        Tree::Node(_, left, kx, vx, right) => match (*left, *right) {
            (Tree::Node(Color::Red, lx, ky, vy, rx), ry) => Tree::Node(
                Color::Red,
                Box::new(Tree::Node(Color::Black, Box::new(l), k, v, lx)),
                ky,
                vy,
                Box::new(Tree::Node(Color::Black, rx, kx, vx, Box::new(ry))),
            ),
            (lx, Tree::Node(Color::Red, ly, ky, vy, ry)) => Tree::Node(
                Color::Red,
                Box::new(Tree::Node(Color::Black, Box::new(l), k, v, Box::new(lx))),
                kx,
                vx,
                Box::new(Tree::Node(Color::Black, ly, ky, vy, ry)),
            ),
            (lx, rx) => Tree::Node(
                Color::Black,
                Box::new(l),
                k,
                v,
                Box::new(Tree::Node(Color::Red, Box::new(lx), kx, vx, Box::new(rx))),
            ),
        },
    }
}

fn ins(tree: Tree, kx: i64, vx: bool) -> Tree {
    match tree {
        Tree::Leaf => Tree::Node(Color::Red, Box::new(Tree::Leaf), kx, vx, Box::new(Tree::Leaf)),
        Tree::Node(Color::Red, a, ky, vy, b) => {
            if kx < ky {
                Tree::Node(Color::Red, Box::new(ins(*a, kx, vx)), ky, vy, b)
            } else if ky < kx {
                Tree::Node(Color::Red, a, ky, vy, Box::new(ins(*b, kx, vx)))
            } else {
                Tree::Node(Color::Red, a, ky, vy, Box::new(ins(*b, kx, vx)))
            }
        }
        Tree::Node(Color::Black, a, ky, vy, b) => {
            if kx < ky {
                if is_red(&a) {
                    balance_left(ins(*a, kx, vx), ky, vy, *b)
                } else {
                    Tree::Node(Color::Black, Box::new(ins(*a, kx, vx)), ky, vy, b)
                }
            } else if ky < kx {
                if is_red(&b) {
                    balance_right(*a, ky, vy, ins(*b, kx, vx))
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

fn set_red(tree: Tree) -> Tree {
    match tree {
        Tree::Node(_, left, key, value, right) => Tree::Node(Color::Red, left, key, value, right),
        other => other,
    }
}

fn make_black(tree: Tree) -> Del {
    match tree {
        Tree::Node(Color::Red, left, key, value, right) => Del(Tree::Node(Color::Black, left, key, value, right), false),
        other => Del(other, true),
    }
}

fn rebalance_left(c: Color, l: Tree, k: i64, v: bool, r: Tree) -> Del {
    match l {
        Tree::Node(Color::Black, _, _, _, _) => Del(balance_left(set_red(l), k, v, r), is_black_color(c)),
        Tree::Node(Color::Red, lx, kx, vx, rx) => Del(Tree::Node(Color::Black, lx, kx, vx, Box::new(balance_left(set_red(*rx), k, v, r))), false),
        _ => Del(Tree::Leaf, false),
    }
}

fn rebalance_right(c: Color, l: Tree, k: i64, v: bool, r: Tree) -> Del {
    match r {
        Tree::Node(Color::Black, _, _, _, _) => Del(balance_right(l, k, v, set_red(r)), is_black_color(c)),
        Tree::Node(Color::Red, lx, kx, vx, rx) => Del(Tree::Node(Color::Black, Box::new(balance_right(l, k, v, set_red(*lx))), kx, vx, rx), false),
        _ => Del(Tree::Leaf, false),
    }
}

fn del_min(tree: Tree) -> Delmin {
    match tree {
        Tree::Node(Color::Black, left, key, value, right) => match *left {
            Tree::Leaf => match *right {
                Tree::Leaf => Delmin(Del(Tree::Leaf, true), key, value),
                right_tree => Delmin(Del(set_black(right_tree), false), key, value),
            },
            left_tree => match del_min(left_tree) {
                Delmin(Del(lx, true), kx, vx) => Delmin(rebalance_right(Color::Black, lx, key, value, *right), kx, vx),
                Delmin(Del(lx, false), kx, vx) => Delmin(Del(Tree::Node(Color::Black, Box::new(lx), key, value, right), false), kx, vx),
            },
        },
        Tree::Node(Color::Red, left, key, value, right) => match *left {
            Tree::Leaf => Delmin(Del(*right, false), key, value),
            left_tree => match del_min(left_tree) {
                Delmin(Del(lx, true), kx, vx) => Delmin(rebalance_right(Color::Red, lx, key, value, *right), kx, vx),
                Delmin(Del(lx, false), kx, vx) => Delmin(Del(Tree::Node(Color::Red, Box::new(lx), key, value, right), false), kx, vx),
            },
        },
        Tree::Leaf => Delmin(Del(Tree::Leaf, false), 0, false),
    }
}

fn del(tree: Tree, key: i64) -> Del {
    match tree {
        Tree::Leaf => Del(Tree::Leaf, false),
        Tree::Node(color, left, kx, vx, right) => {
            if key < kx {
                match del(*left, key) {
                    Del(ly, true) => rebalance_right(color, ly, kx, vx, *right),
                    Del(ly, false) => Del(Tree::Node(color, Box::new(ly), kx, vx, right), false),
                }
            } else if key > kx {
                match del(*right, key) {
                    Del(ry, true) => rebalance_left(color, *left, kx, vx, ry),
                    Del(ry, false) => Del(Tree::Node(color, left, kx, vx, Box::new(ry)), false),
                }
            } else {
                match *right {
                    Tree::Leaf => {
                        if is_black_color(color) {
                            make_black(*left)
                        } else {
                            Del(*left, false)
                        }
                    }
                    right_tree => match del_min(right_tree) {
                        Delmin(Del(ry, true), ky, vy) => rebalance_left(color, *left, ky, vy, ry),
                        Delmin(Del(ry, false), ky, vy) => Del(Tree::Node(color, left, ky, vy, Box::new(ry)), false),
                    },
                }
            }
        }
    }
}

fn delete(tree: Tree, key: i64) -> Tree {
    match del(tree, key) {
        Del(next, _) => set_black(next),
    }
}

fn mk_map_aux(total: i64, n: i64, tree: Tree) -> Tree {
    if n == 0 {
        tree
    } else {
        let n1 = n - 1;
        let t1 = insert(tree, n1, n1 % 10 == 0);
        let t2 = if n1 % 4 == 0 { delete(t1, n1 + (total - n1) / 5) } else { t1 };
        mk_map_aux(total, n1, t2)
    }
}

fn main() {
    let handle = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            println!("{}", count_true(&mk_map_aux(LIMIT, LIMIT, Tree::Leaf)));
        })
        .expect("spawn rbtree_del benchmark");
    handle.join().expect("run rbtree_del benchmark");
}
