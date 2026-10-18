//! `reorder`: shuffle the top-level declarations. A compiler that lowers in
//! definition order, or in an order it got from a hash, gives the copy a
//! different program.

use super::Rng;
use super::items::Fixture;

/// The fixture with its movable items shuffled, each within the run of
/// items between two pinned ones; the header, imports and top-level `let`s
/// stay where they are. An item never crosses a `let`: whether a function
/// may read a `let` written after it is the language's rule to make, not a
/// rewrite's. `None` when no run has two items.
pub fn reorder(fixture: &Fixture, rng: &mut Rng) -> Option<String> {
    let items = &fixture.items;
    let indices: Vec<usize> = (0..items.len()).collect();
    let runs: Vec<&[usize]> = indices
        .split(|&i| !items[i].is_movable())
        .filter(|run| run.len() > 1)
        .collect();
    if runs.is_empty() {
        return None;
    }
    let mut placed = indices.clone();
    // A draw can be the order already written; with two items it is half
    // the time.
    for _ in 0..8 {
        for run in &runs {
            let mut order = run.to_vec();
            rng.shuffle(&mut order);
            for (&slot, &item) in run.iter().zip(&order) {
                placed[slot] = item;
            }
        }
        if placed != indices {
            break;
        }
    }
    if placed == indices {
        return None;
    }
    let source = fixture.source;
    let mut out = source[..fixture.header_end].to_string();
    for index in placed {
        let text = &source[items[index].start..items[index].end];
        out.push_str(text);
        if !text.ends_with('\n') {
            out.push('\n');
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rewrite::same_tokens;

    const SOURCE: &str = "// expect: success\nimport Flow.List as L\nfn a() -> Int { 1 }\nlet x = a()\nfn b() -> Int { 2 }\ndata D { D(Int) }\nfn main() { print(x) }\n";

    fn reordered(seed: u64) -> String {
        reorder(&Fixture::parse(SOURCE).unwrap(), &mut Rng::new(seed)).unwrap()
    }

    #[test]
    fn the_header_imports_and_lets_stay_in_their_slots() {
        for seed in 0..20 {
            let out = reordered(seed);
            let lines: Vec<&str> = out.lines().collect();
            assert_eq!(lines[0], "// expect: success");
            assert_eq!(lines[1], "import Flow.List as L");
            assert_eq!(lines[3], "let x = a()");
            assert_ne!(out, SOURCE);
            assert!(same_tokens(SOURCE, &out));
        }
    }

    #[test]
    fn a_seed_gives_one_order() {
        assert_eq!(reordered(7), reordered(7));
        assert!((0..20).any(|seed| reordered(seed) != reordered(7)));
    }

    #[test]
    fn no_item_crosses_a_let() {
        for seed in 0..20 {
            let out = reordered(seed);
            let lines: Vec<&str> = out.lines().collect();
            assert_eq!(lines[2], "fn a() -> Int { 1 }");
        }
    }

    #[test]
    fn one_movable_item_is_left_alone() {
        let fixture = Fixture::parse("import Flow.List as L\nfn main() {}\n").unwrap();
        assert_eq!(reorder(&fixture, &mut Rng::new(1)), None);
    }
}
