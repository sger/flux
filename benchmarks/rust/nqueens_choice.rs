// Port of benchmarks/flux/nqueens_choice_direct.flx: Rust has no effect
// handlers, so every row is tried in a loop instead of by resuming `pick`.
use std::env;
use std::rc::Rc;

enum List {
    Nil,
    Cons(i64, Rc<List>),
}

fn safe(queen: i64, diag: i64, xs: &List) -> bool {
    match xs {
        List::Nil => true,
        List::Cons(q, qs) => {
            queen != *q && queen != q + diag && queen != q - diag && safe(queen, diag + 1, qs)
        }
    }
}

fn place(size: i64, column: i64, rest: &Rc<List>) -> i64 {
    if column > size {
        return 1;
    }
    let mut a = 0;
    for row in 1..=size {
        if safe(row, 1, rest) {
            a += place(size, column + 1, &Rc::new(List::Cons(row, Rc::clone(rest))));
        }
    }
    a
}

fn main() {
    let n = env::args().nth(1).and_then(|value| value.parse::<i64>().ok()).unwrap_or(12);
    println!("{}", place(n, 1, &Rc::new(List::Nil)));
}
