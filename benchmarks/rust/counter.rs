// Port of benchmarks/flux/counter_direct.flx: Rust has no effect
// handlers, so the state is the loop's counter, beside the step count.
use std::env;

fn counter(mut i: i64) -> i64 {
    let mut c = 0;
    while i != 0 {
        i -= 1;
        c += 1;
    }
    c
}

fn main() {
    let n = env::args().nth(1).and_then(|value| value.parse::<i64>().ok()).unwrap_or(100100100);
    println!("{}", counter(n));
}
