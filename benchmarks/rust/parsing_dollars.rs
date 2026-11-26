// Port of benchmarks/flux/parsing_dollars_direct.flx: Rust has no effect
// handlers, so each char is made from the two counters where it is read,
// and the two mutually recursive functions are one loop.
use std::env;

fn run(n: i64) -> i64 {
    let (mut i, mut j, mut a, mut s) = (0i64, 0i64, 0i64, 0i64);
    loop {
        if i > n {
            return s;
        }
        let c = if j == 0 {
            i += 1;
            j = i;
            10
        } else {
            j -= 1;
            36
        };
        if c == 36 {
            a += 1;
        } else if c == 10 {
            s += a;
            a = 0;
        } else {
            return s;
        }
    }
}

fn main() {
    let n = env::args().nth(1).and_then(|value| value.parse::<i64>().ok()).unwrap_or(20000);
    println!("{}", run(n));
}
