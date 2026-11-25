// Port of benchmarks/flux/iterator_direct.flx: Rust has no effect
// handlers, so each emitted value is added where it is made.
use std::env;

fn range(l: i64, u: i64) -> i64 {
    let mut s = 0;
    let mut i = l;
    while i <= u {
        s += i;
        i += 1;
    }
    s
}

fn main() {
    let n = env::args().nth(1).and_then(|value| value.parse::<i64>().ok()).unwrap_or(40000000);
    println!("{}", range(0, n));
}
