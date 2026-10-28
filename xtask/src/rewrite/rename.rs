//! `rename`: give local binders fresh names. A compiler that looks a local up
//! by name where it should follow the binder, or that lets one name capture
//! another, gives the copy a different program.
//!
//! The rename is by name within one item: every occurrence of a renamed name
//! in the item changes, so a fixture that shadows on purpose keeps its
//! scoping. Only names that are certainly local are renamed; anything that
//! could also name a field, a declaration or an import is left alone.

use std::collections::{BTreeSet, HashSet};

use super::items::Fixture;
use super::lex::{Kind, Token, is_keyword};

/// The fixture with the local binders of its functions, classes and
/// instances renamed; `None` when none can be.
pub fn rename(fixture: &Fixture<'_>) -> Option<String> {
    let source = fixture.source;
    let excluded = excluded_names(fixture);
    let taken: HashSet<&str> =
        fixture.tokens.iter().filter(|t| t.kind == Kind::Ident).map(|t| t.text(source)).collect();
    let mut fresh = 0;
    let mut edits: Vec<(Token, String)> = Vec::new();
    for item in fixture.items.iter().filter(|item| item.has_code()) {
        let code = fixture.code_in(item);
        // A name used before it is bound refers to something outside.
        let mut renames = Vec::new();
        for (name, first) in binders(source, &code) {
            let used_before = (0..first).any(|i| is_use(source, &code, i, name));
            if excluded.contains(name) || used_before || !is_local_name(name) {
                continue;
            }
            let new_name = loop {
                let candidate = format!("rw{fresh}_{name}");
                fresh += 1;
                if !taken.contains(candidate.as_str()) {
                    break candidate;
                }
            };
            renames.push((name, new_name));
        }
        for (name, new_name) in renames {
            for (index, token) in code.iter().enumerate() {
                if is_use(source, &code, index, name) {
                    edits.push((*token, new_name.clone()));
                }
            }
        }
    }
    if edits.is_empty() {
        return None;
    }
    edits.sort_by_key(|(token, _)| token.start);
    let mut out = String::with_capacity(source.len() + edits.len() * 4);
    let mut at = 0;
    for (token, text) in edits {
        out.push_str(&source[at..token.start]);
        out.push_str(&text);
        at = token.end;
    }
    out.push_str(&source[at..]);
    Some(out)
}

/// Whether `code[index]` is `name` as a variable, not as a field after `.`.
fn is_use(source: &str, code: &[Token], index: usize, name: &str) -> bool {
    code[index].is(source, name) && (index == 0 || !code[index - 1].is(source, "."))
}

fn is_local_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_lowercase()) && !is_keyword(name)
}

/// Each name `code` binds, with the index of its first binding: `let`
/// patterns, the parameters of a `fn` or `fn(…)`, and lambda parameters.
fn binders<'a>(source: &'a str, code: &[Token]) -> Vec<(&'a str, usize)> {
    let text = |i: usize| code.get(i).map_or("", |t| t.text(source));
    let mut found: Vec<(&str, usize)> = Vec::new();
    let mut bind = |i: usize| {
        let name = code[i].text(source);
        if code[i].kind == Kind::Ident && !found.iter().any(|(n, _)| *n == name) {
            found.push((name, i));
        }
    };
    for i in 0..code.len() {
        match text(i) {
            // `let (a, b) = …`, `\(x: T) -> …`
            "let" | "\\" if text(i + 1) == "(" => {
                for j in tuple_binders(source, code, i + 1) {
                    bind(j);
                }
            }
            "let" => bind(i + 1),
            // `fn name(…)`, `fn name<a>(…)`, `fn(…)`
            "fn" => {
                let open = (i + 1..code.len())
                    .find(|&j| matches!(text(j), "(" | "{" | "->" | ""))
                    .unwrap_or(code.len());
                if text(open) == "(" {
                    for j in tuple_binders(source, code, open) {
                        bind(j);
                    }
                }
            }
            // `\x -> …`, `\x, y -> …`
            "\\" => {
                let mut j = i + 1;
                while code.get(j).is_some_and(|t| t.kind == Kind::Ident) {
                    bind(j);
                    if text(j + 1) != "," {
                        break;
                    }
                    j += 2;
                }
            }
            _ => {}
        }
    }
    found
}

/// The names a parenthesised parameter list or tuple pattern at `open`
/// binds: a name right after `(` or `,` at its own depth, followed by `:`,
/// `,` or `)`. A type written there, as in `(f: (a) -> b)`, is one level
/// deeper and so is not taken.
fn tuple_binders(source: &str, code: &[Token], open: usize) -> Vec<usize> {
    let mut depth = 0;
    let mut names = Vec::new();
    for i in open..code.len() {
        match code[i].text(source) {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ if depth == 1 && code[i].kind == Kind::Ident => {
                let before = code[i - 1].text(source);
                let after = code.get(i + 1).map_or("", |t| t.text(source));
                if matches!(before, "(" | ",") && matches!(after, ":" | "," | ")") {
                    names.push(i);
                }
            }
            _ => {}
        }
    }
    names
}

/// Names a rename must leave alone anywhere in the file: what the file
/// declares (functions, types, methods, top-level `let`s), and what could be
/// a field — a name before `:` in braces, or a punned field.
fn excluded_names<'a>(fixture: &Fixture<'a>) -> BTreeSet<&'a str> {
    let source = fixture.source;
    let code: Vec<Token> = fixture.tokens.iter().copied().filter(|t| t.is_code()).collect();
    let text = |i: usize| code.get(i).map_or("", |t| t.text(source));
    let mut names = BTreeSet::new();
    // For each open bracket: its text, and whether it opens a record (after a
    // constructor name, or starting with a spread).
    let mut open: Vec<(&str, bool)> = Vec::new();
    for i in 0..code.len() {
        let word = text(i);
        match word {
            "fn" | "data" | "type" | "effect" | "alias" | "class" | "instance" => {
                names.insert(text(i + 1));
            }
            "{" => {
                let after_constructor = i > 0
                    && code[i - 1].kind == Kind::Ident
                    && text(i - 1).starts_with(|c: char| c.is_ascii_uppercase());
                open.push(("{", after_constructor || text(i + 1) == "..."));
            }
            "(" | "[" => open.push((word, false)),
            ")" | "]" | "}" => {
                open.pop();
            }
            _ => {}
        }
        if code[i].kind != Kind::Ident || i == 0 {
            continue;
        }
        let Some(&(bracket, record)) = open.last() else {
            continue;
        };
        let starts_entry = matches!(text(i - 1), "{" | ",");
        let field = bracket == "{" && starts_entry && text(i + 1) == ":";
        let punned = record && starts_entry && matches!(text(i + 1), "," | "}");
        if field || punned {
            names.insert(word);
        }
    }
    for item in fixture.items.iter().filter(|item| item.keyword == "let") {
        if let Some(name) = fixture.code_in(item).get(1) {
            names.insert(name.text(source));
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn renamed(source: &str) -> String {
        rename(&Fixture::parse(source).unwrap()).unwrap_or_else(|| source.to_string())
    }

    #[test]
    fn parameters_lets_and_lambda_binders_get_fresh_names() {
        let out = renamed("fn f(x: Int, y) -> Int {\n  let z = x + y\n  g(\\w -> w + z)\n}\n");
        assert_eq!(
            out,
            "fn f(rw0_x: Int, rw1_y) -> Int {\n  let rw2_z = rw0_x + rw1_y\n  g(\\rw3_w -> rw3_w + rw2_z)\n}\n"
        );
    }

    #[test]
    fn a_name_inside_an_interpolation_is_renamed() {
        assert_eq!(renamed("fn f(n) { \"n=#{n}\" }\n"), "fn f(rw0_n) { \"n=#{rw0_n}\" }\n");
    }

    #[test]
    fn fields_punning_and_declarations_are_left_alone() {
        let source = "data P { P { name: String } }\nfn helper(v) { v }\nfn main() {\n  let name = \"a\"\n  let p = P { name }\n  let helper = 1\n  print(p.name)\n}\n";
        let out = renamed(source);
        assert!(out.contains("let name = \"a\""), "{out}");
        assert!(out.contains("let helper = 1"), "{out}");
        assert!(out.contains("p.name"), "{out}");
        assert!(out.contains("let rw"), "{out}");
    }

    #[test]
    fn a_name_used_before_its_binding_is_left_alone() {
        let out = renamed("fn main() {\n  print(len)\n  let len = 2\n}\n");
        assert!(out.contains("let len = 2"), "{out}");
    }

    #[test]
    fn a_shadowed_name_is_renamed_everywhere_alike() {
        assert_eq!(
            renamed("fn main() {\n  let a = 1\n  let a = a + 1\n  print(a)\n}\n"),
            "fn main() {\n  let rw0_a = 1\n  let rw0_a = rw0_a + 1\n  print(rw0_a)\n}\n"
        );
    }

    #[test]
    fn a_type_in_a_parameter_list_is_not_a_binder() {
        let out = renamed("fn f(g: (a) -> a, x: a) -> a { g(x) }\n");
        assert_eq!(out, "fn f(rw0_g: (a) -> a, rw1_x: a) -> a { rw0_g(rw1_x) }\n");
    }
}
