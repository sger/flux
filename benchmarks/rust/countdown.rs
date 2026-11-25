// Port of benchmarks/flux/countdown_direct.flx: Rust has no effect
// handlers, so the state is the loop's counter.
use std::env;

fn countdown(mut i: i64) -> i64 {
    while i != 0 {
        i -= 1;
    }
    i
}

fn main() {
    let n = env::args().nth(1).and_then(|value| value.parse::<i64>().ok()).unwrap_or(200000000);
    println!("{}", countdown(n));
}
