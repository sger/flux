// Port of benchmarks/flux/triples_direct.flx: Rust has no effect handlers,
// so the triples i > j > k that sum to n are visited in nested loops and
// their hashes added as they are found.
use std::env;

const M: i64 = 1_000_000_007;

fn run(n: i64) -> i64 {
    let mut acc = 0;
    for i in (1..=n).rev() {
        for j in (1..i).rev() {
            for k in (1..j).rev() {
                if i + j + k == n {
                    acc = (acc + (53 * i + 2809 * j + 148877 * k) % M) % M;
                }
            }
        }
    }
    acc
}

fn main() {
    let n = env::args().nth(1).and_then(|value| value.parse::<i64>().ok()).unwrap_or(10);
    println!("{}", run(n));
}
