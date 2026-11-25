use std::env;

fn sfib(mut n: i64) -> i32 {
    let (mut x1, mut x2): (i32, i32) = (1, 2);
    while n > 0 {
        if x2 > 1_000_000_000 {
            (x1, x2) = (1, 2);
        } else {
            (x1, x2) = (x2.wrapping_sub(x1), x1.wrapping_mul(x2).wrapping_add(x1));
        }
        n -= 1;
    }
    x1
}

fn main() {
    let n = env::args().nth(1).and_then(|value| value.parse::<i64>().ok()).unwrap_or(20_000_000);
    println!("{}", sfib(n));
}
