use std::env;
use std::fs::File;
use std::io::{self, BufRead, BufReader};

fn count_hits_for_rotation(first_zero_click: i64, distance: i64) -> i64 {
    if distance < first_zero_click {
        0
    } else {
        1 + (distance - first_zero_click) / 100
    }
}

fn count_zeros_right(pos: i64, distance: i64) -> i64 {
    let first_zero_click = if pos == 0 { 100 } else { 100 - pos };
    count_hits_for_rotation(first_zero_click, distance)
}

fn count_zeros_left(pos: i64, distance: i64) -> i64 {
    let first_zero_click = if pos == 0 { 100 } else { pos };
    count_hits_for_rotation(first_zero_click, distance)
}

fn solve(path: &str) -> io::Result<i64> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let mut pos: i64 = 50;
    let mut hits: i64 = 0;

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (direction, dist_str) = trimmed.split_at(1);
        let dist: i64 = dist_str
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid distance"))?;

        match direction {
            "L" => {
                hits += count_zeros_left(pos, dist);
                pos = (pos - dist).rem_euclid(100);
            }
            "R" => {
                hits += count_zeros_right(pos, dist);
                pos = (pos + dist).rem_euclid(100);
            }
            _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid rotation")),
        }
    }

    Ok(hits)
}

fn main() {
    let mut args = env::args();
    let _ = args.next();
    let Some(path) = args.next() else {
        eprintln!("usage: day1_part2_rust <input-file>");
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
