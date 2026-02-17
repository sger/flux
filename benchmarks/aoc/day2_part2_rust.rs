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

fn repeat_value(seed: i64, base: i64, times: u32) -> i64 {
    let mut acc = 0_i64;
    let mut i = 0_u32;
    while i < times {
        acc = acc * base + seed;
        i += 1;
    }
    acc
}

fn has_repeated_pattern_len(value: i64, total_len: u32, chunk_len: u32) -> bool {
    if !total_len.is_multiple_of(chunk_len) {
        return false;
    }

    let repeats = total_len / chunk_len;
    if repeats < 2 {
        return false;
    }

    let base = 10_i64.pow(chunk_len);
    let seed = value / 10_i64.pow(total_len - chunk_len);
    repeat_value(seed, base, repeats) == value
}

fn is_invalid_id(value: i64) -> bool {
    let total_len = digits_count(value);
    let mut chunk_len = 1_u32;
    while chunk_len * 2 <= total_len {
        if has_repeated_pattern_len(value, total_len, chunk_len) {
            return true;
        }
        chunk_len += 1;
    }
    false
}

fn solve(path: &str) -> io::Result<i64> {
    let text = fs::read_to_string(path)?;

    let mut ranges: Vec<(i64, i64)> = Vec::new();
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
    }

    let mut total = 0_i64;
    for (start, end) in ranges {
        let mut value = start;
        while value <= end {
            if is_invalid_id(value) {
                total += value;
            }
            value += 1;
        }
    }

    Ok(total)
}

fn main() {
    let mut args = env::args();
    let _ = args.next();
    let Some(path) = args.next() else {
        eprintln!("usage: day2_part2_rust <input-file>");
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
