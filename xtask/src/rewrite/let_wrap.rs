//! `let-wrap`: bind a call's argument to a fresh name first, as
//! `f(do { let t = a; t })`. Evaluation order and effects stay the same; the
//! shape changes, which is what the optimizer's inliner and the ownership
//! pass see.

use std::fmt::Write as _;

use super::Rng;
use super::items::Fixture;
use super::lex::{Kind, Token, is_keyword};

/// At most this many wraps per file, so a failing copy stays readable.
const MAX_WRAPS: usize = 8;

/// The fixture with up to [`MAX_WRAPS`] call arguments wrapped, chosen by
/// `rng`; `None` when no call qualifies.
pub fn let_wrap(fixture: &Fixture<'_>, rng: &mut Rng) -> Option<String> {
    let source = fixture.source;
    let mut candidates: Vec<(usize, usize)> = Vec::new();
    for item in fixture.items.iter().filter(|item| item.has_code()) {
        let code = fixture.code_in(item);
        for call in calls(source, &code) {
            let arguments = arguments(source, &code, call);
            if arguments.is_empty() {
                continue;
            }
            let (first, last) = arguments[rng.below(arguments.len())];
            // A lambda takes its parameter types from the call; bound by
            // `let` first, it would lose them.
            if matches!(code[first].text(source), "\\" | "fn") {
                continue;
            }
            candidates.push((code[first].start, code[last].end));
        }
    }
    rng.shuffle(&mut candidates);
    let mut chosen: Vec<(usize, usize)> = Vec::new();
    for (start, end) in candidates {
        if chosen.len() == MAX_WRAPS {
            break;
        }
        if chosen.iter().all(|&(s, e)| end <= s || start >= e) {
            chosen.push((start, end));
        }
    }
    if chosen.is_empty() {
        return None;
    }
    chosen.sort_unstable();
    let taken = |name: &str| fixture.tokens.iter().any(|t| t.is(source, name));
    let mut names = (0..).map(|n| format!("rw{n}")).filter(|name| !taken(name));
    let mut out = String::with_capacity(source.len() + chosen.len() * 24);
    let mut at = 0;
    for (start, end) in chosen {
        let name = names.next().expect("names never run out");
        out.push_str(&source[at..start]);
        let _ = write!(out, "do {{ let {name} = {}; {name} }}", &source[start..end]);
        at = end;
    }
    out.push_str(&source[at..]);
    Some(out)
}

/// The index of each `(` in `code` that calls a function from inside a body:
/// after a lowercase name that is not being declared, and not the head of a
/// pattern or a handler arm (`op(resume, x) -> …`) or of an interpolation.
fn calls(source: &str, code: &[Token]) -> Vec<usize> {
    let text = |i: usize| code.get(i).map_or("", |t| t.text(source));
    let mut braces = 0;
    let mut found = Vec::new();
    for i in 0..code.len() {
        match text(i) {
            "{" => braces += 1,
            "}" => braces -= 1,
            "(" if braces > 0 && i > 0 => {
                let callee = code[i - 1];
                let word = callee.text(source);
                // `let (a, b) = …` is a pattern; `resume(x)` is a call.
                let named = callee.kind == Kind::Ident
                    && word.starts_with(|c: char| c.is_ascii_lowercase())
                    && (!is_keyword(word) || word == "resume");
                let declared = i > 1 && text(i - 2) == "fn";
                let close = closing(source, code, i);
                let pattern = close.is_some_and(|c| matches!(text(c + 1), "->" | "=>"));
                if named && !declared && !pattern && !code[i].in_interpolation {
                    found.push(i);
                }
            }
            _ => {}
        }
    }
    found
}

fn closing(source: &str, code: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, token) in code.iter().enumerate().skip(open) {
        match token.text(source) {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// The first and last token of each argument of the call at `open`.
fn arguments(source: &str, code: &[Token], open: usize) -> Vec<(usize, usize)> {
    let Some(close) = closing(source, code, open) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut depth = 0;
    let mut first = open + 1;
    for (i, token) in code.iter().enumerate().take(close + 1).skip(open + 1) {
        match token.text(source) {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" if i < close => depth -= 1,
            "," if depth == 0 => {
                if first < i {
                    found.push((first, i - 1));
                }
                first = i + 1;
            }
            _ => {}
        }
    }
    if first < close {
        found.push((first, close - 1));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrapped(source: &str, seed: u64) -> Option<String> {
        let_wrap(&Fixture::parse(source).unwrap(), &mut Rng::new(seed))
    }

    #[test]
    fn an_argument_is_bound_to_a_fresh_name() {
        assert_eq!(
            wrapped("fn main() {\n  print(1 + 2)\n}\n", 1).unwrap(),
            "fn main() {\n  print(do { let rw0 = 1 + 2; rw0 })\n}\n"
        );
    }

    #[test]
    fn declarations_patterns_and_handler_arms_are_not_calls() {
        let source = "fn f(x) { x }\nfn main() {\n  let r = f(1) handle E {\n    op(resume, n) -> resume(n)\n  }\n  match r { Some(v) -> v, _ -> 0 }\n}\n";
        let out = wrapped(source, 3).unwrap();
        assert!(out.starts_with("fn f(x) { x }"), "{out}");
        assert!(out.contains("op(resume, n) ->"), "{out}");
        assert!(out.contains("Some(v)"), "{out}");
        assert!(out.contains("f(do { let rw0 = 1; rw0 })"), "{out}");
    }

    #[test]
    fn a_let_pattern_is_not_a_call() {
        assert_eq!(wrapped("fn main() {\n  let (x, y) = p\n}\n", 1), None);
    }

    #[test]
    fn lambdas_and_empty_calls_are_left_alone() {
        assert_eq!(wrapped("fn main() {\n  g()\n  map(\\x -> x)\n}\n", 1), None);
    }

    #[test]
    fn the_number_of_wraps_is_capped() {
        let calls = "  print(1)\n".repeat(20);
        let out = wrapped(&format!("fn main() {{\n{calls}}}\n"), 5).unwrap();
        assert_eq!(out.matches("do {").count(), MAX_WRAPS);
    }

    #[test]
    fn a_seed_picks_the_same_wraps() {
        let calls = "  print(1)\n".repeat(20);
        let source = format!("fn main() {{\n{calls}}}\n");
        assert_eq!(wrapped(&source, 9), wrapped(&source, 9));
    }
}
