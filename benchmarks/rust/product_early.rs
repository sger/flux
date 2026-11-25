// Port of benchmarks/flux/product_early_direct.flx: Rust has no effect
// handlers, so the product returns 0 at the first zero.
use std::env;

enum List {
    Nil,
    Cons(i64, Box<List>),
}

fn product(xs: &List) -> i64 {
    match xs {
        List::Nil => 0,
        List::Cons(y, ys) => {
            if *y == 0 {
                0
            } else {
                y * product(ys)
            }
        }
    }
}

fn enumerate(i: i64) -> List {
    let mut xs = List::Nil;
    for v in 0..=i {
        xs = List::Cons(v, Box::new(xs));
    }
    xs
}

fn main() {
    let n = env::args().nth(1).and_then(|value| value.parse::<i64>().ok()).unwrap_or(100000);
    let xs = enumerate(1000);
    let mut a = 0;
    for _ in 0..n {
        a += product(&xs);
    }
    println!("{a}");
}
