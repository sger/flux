// Port of benchmarks/flux/fibonacci_recursive.flx.
use std::env;

fn fibonacci(n: i64) -> i64 {
    if n == 0 {
        0
    } else if n == 1 {
        1
    } else {
        fibonacci(n - 1) + fibonacci(n - 2)
    }
}

fn main() {
    let n = env::args().nth(1).and_then(|value| value.parse::<i64>().ok()).unwrap_or(42);
    println!("{}", fibonacci(n));
}
