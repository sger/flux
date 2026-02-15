use std::env;
use std::fs;
use std::io;

fn digits_count(mut n: i64) -> u32 {
    let mut c = 1;
    while n >= 10 {
        n /= 10;
        c += 1;
    }
    c
}

fn invalid_from_seed(seed: i64) -> i64 {
    let base = 10_i64.pow(digits_count(seed));
    seed * (base + 1)
}

fn seed_limit_for_max_id(max_id: i64) -> i64 {
    let max_digits = digits_count(max_id);
    let seed_digits = (max_digits + 1) / 2;
    10_i64.pow(seed_digits)
}

fn solve(path: &str) -> io::Result<i64> {
    let text = fs::read_to_string(path)?;

    let mut ranges: Vec<(i64, i64)> = Vec::new();
    let mut max_end = 0_i64;

    for token in text.trim().split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let (s, e) = token
            .split_once('-')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid range token"))?;
        let start: i64 = s
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid start"))?;
        let end: i64 = e
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid end"))?;
        ranges.push((start, end));
        if end > max_end {
            max_end = end;
        }
    }

    let mut total = 0_i64;
    let limit = seed_limit_for_max_id(max_end);
    let mut seed = 1_i64;
    while seed < limit {
        let candidate = invalid_from_seed(seed);
        if ranges
            .iter()
            .any(|(start, end)| *start <= candidate && candidate <= *end)
        {
            total += candidate;
        }
        seed += 1;
    }

    Ok(total)
}

fn main() {
    let mut args = env::args();
    let _ = args.next();
    let Some(path) = args.next() else {
        eprintln!("usage: day2_part1_rust <input-file>");
        std::process::exit(2);
    };

    match solve(&path) {
        Ok(ans) => println!("{ans}"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
