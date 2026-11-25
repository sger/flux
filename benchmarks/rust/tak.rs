use std::env;

fn tak(x: i64, y: i64, z: i64) -> i64 {
    if y < x { tak(tak(x - 1, y, z), tak(y - 1, z, x), tak(z - 1, x, y)) } else { z }
}

fn main() {
    let n = env::args().nth(1).and_then(|value| value.parse::<i64>().ok()).unwrap_or(30);
    println!("{}", tak(n, n * 2 / 3, n / 3));
}
