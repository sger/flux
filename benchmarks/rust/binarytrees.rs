use std::env;

#[derive(Clone)]
enum Tree {
    Node(Box<Tree>, Box<Tree>),
    Tip,
}

fn make_tree(depth: i32) -> Tree {
    if depth > 0 {
        Tree::Node(
            Box::new(make_tree(depth - 1)),
            Box::new(make_tree(depth - 1)),
        )
    } else {
        Tree::Node(Box::new(Tree::Tip), Box::new(Tree::Tip))
    }
}

fn check_tree(tree: &Tree) -> i32 {
    match tree {
        Tree::Node(left, right) => check_tree(left) + check_tree(right) + 1,
        Tree::Tip => 0,
    }
}

fn sum_trees(iterations: i32, depth: i32) -> i32 {
    let mut total = 0;
    for _ in 0..iterations {
        let tree = make_tree(depth);
        total += check_tree(&tree);
    }
    total
}

fn main() {
    let n = env::args()
        .nth(1)
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(21);
    let min_depth = 4;
    let max_depth = std::cmp::max(min_depth + 2, n);
    let stretch_depth = max_depth + 1;

    println!(
        "stretch tree of depth {}\t check: {}",
        stretch_depth,
        check_tree(&make_tree(stretch_depth))
    );

    let long_lived_tree = make_tree(max_depth);

    for depth in (min_depth..=max_depth).step_by(2) {
        let iterations = 1_i32 << (max_depth + min_depth - depth);
        let total = sum_trees(iterations, depth);
        println!(
            "{}\t trees of depth {}\t check: {}",
            iterations, depth, total
        );
    }

    println!(
        "long lived tree of depth {}\t check: {}",
        max_depth,
        check_tree(&long_lived_tree)
    );
}
