// Port of benchmarks/flux/resume_nontail_direct.flx: Rust has no effect
// handlers, so the work after each resume is the work after a call.
use std::env;

fn combine(x: i64, y: i64) -> i64 {
    (x - 503 * y + 37).abs() % 1009
}

fn run(i: i64, s: i64) -> i64 {
    if i == 0 { s } else { combine(i, run(i - 1, s)) }
}

fn main() {
    let n = env::args().nth(1).and_then(|value| value.parse::<i64>().ok()).unwrap_or(10000);
    let mut s = 0;
    for _ in 0..1000 {
        s = run(n, s);
    }
    println!("{s}");
}
