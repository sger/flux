//! A fixture split into its header and its top-level items, which is what
//! every rewrite works on.

use super::lex::{self, Kind, Token};

/// A fixture's source, its tokens, and where its header and items are.
pub struct Fixture<'a> {
    pub source: &'a str,
    pub tokens: Vec<Token>,
    /// Where the header ends: parity-check reads directives only from the
    /// leading run of blank and `//` lines (`src/parity/fixture.rs`), so it
    /// stays first whatever moves.
    pub header_end: usize,
    pub items: Vec<Item>,
}

/// One top-level item, with the comments and blank lines before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Its byte range; the next item starts where this one ends.
    pub start: usize,
    pub end: usize,
    /// What it declares: `fn`, `data`, `import`, `let`, …, after any
    /// `public`, `intrinsic` or `@annotation`.
    pub keyword: String,
}

impl Item {
    /// Whether moving it cannot change the program: declarations are
    /// collected before anything runs, but a top-level `let` runs where it
    /// is (`src/binding_groups.rs`), and imports stay at the top.
    pub fn is_movable(&self) -> bool {
        !matches!(self.keyword.as_str(), "import" | "let" | "module")
    }

    /// Whether its body is code a rename or a let-wrap may change.
    pub fn has_code(&self) -> bool {
        matches!(self.keyword.as_str(), "fn" | "instance" | "class")
    }
}

const ITEM_KEYWORDS: [&str; 10] = [
    "fn", "data", "type", "effect", "alias", "class", "instance", "import", "let", "module",
];

impl<'a> Fixture<'a> {
    pub fn parse(source: &'a str) -> Result<Fixture<'a>, String> {
        let tokens = lex::tokenize(source)?;
        let header_end = header_end(source);
        let items = split(source, &tokens, header_end)?;
        Ok(Fixture {
            source,
            tokens,
            header_end,
            items,
        })
    }

    /// The code tokens of `item`.
    pub fn code_in(&self, item: &Item) -> Vec<Token> {
        self.tokens
            .iter()
            .copied()
            .filter(|t| t.start >= item.start && t.end <= item.end && t.is_code())
            .collect()
    }

    /// Whether the program has a `fn main`.
    pub fn has_main(&self) -> bool {
        self.tokens
            .windows(2)
            .any(|w| w[0].is(self.source, "fn") && w[1].is(self.source, "main"))
    }
}

/// The offset of the first line that is neither blank nor a `//` comment.
fn header_end(source: &str) -> usize {
    let mut at = 0;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with("//") {
            break;
        }
        at += line.len();
    }
    at
}

/// Splits the source after `header_end` into items. An item starts at a
/// line whose first token, at brace depth 0, is an item keyword; the comment
/// lines right before it belong to it. Any other line continues the item
/// before, which keeps a multi-line signature, a `let` initializer and a
/// `type … | …` whole.
fn split(source: &str, tokens: &[Token], header_end: usize) -> Result<Vec<Item>, String> {
    let mut items: Vec<Item> = Vec::new();
    let mut depth: i32 = 0;
    let mut comments_from: Option<usize> = None;
    let mut previous_end = 0;
    for (index, token) in tokens.iter().enumerate() {
        let gap = &source[previous_end..token.start];
        let line_start = gap
            .rfind('\n')
            .map_or(previous_end, |n| previous_end + n + 1);
        let first_on_line = token.start >= header_end && (index == 0 || gap.contains('\n'));
        previous_end = token.end;
        if first_on_line && depth == 0 {
            if !token.is_code() {
                comments_from.get_or_insert(line_start);
            } else if let Some(keyword) = item_keyword(source, &tokens[index..]) {
                let start = comments_from.take().unwrap_or(line_start);
                if let Some(last) = items.last_mut() {
                    last.end = start;
                }
                items.push(Item {
                    start: if items.is_empty() { header_end } else { start },
                    end: source.len(),
                    keyword,
                });
            } else if items.is_empty() {
                return Err("code before the first top-level item".to_string());
            } else {
                comments_from = None;
            }
        }
        if token.kind == Kind::Punct && !token.in_interpolation {
            match token.text(source) {
                "{" | "(" | "[" => depth += 1,
                "}" | ")" | "]" => depth -= 1,
                _ => {}
            }
        }
        if depth < 0 {
            return Err(format!(
                "unbalanced `{}` at byte {}",
                token.text(source),
                token.start
            ));
        }
    }
    if depth != 0 {
        return Err("unbalanced brackets at the end of the file".to_string());
    }
    Ok(items)
}

/// The declaring keyword when `tokens` starts an item.
fn item_keyword(source: &str, tokens: &[Token]) -> Option<String> {
    let mut code = tokens
        .iter()
        .filter(|t| t.is_code())
        .map(|t| t.text(source));
    let mut word = code.next()?;
    loop {
        match word {
            // `@fip fn …`
            "@" => {
                code.next()?;
                word = code.next()?;
            }
            "public" | "intrinsic" => word = code.next()?,
            _ => break,
        }
    }
    if !ITEM_KEYWORDS.contains(&word) {
        return None;
    }
    // `fn (x) { … }` at the top would be an expression, not an item.
    let named = code
        .next()
        .is_some_and(|name| name.starts_with(|c: char| c.is_alphabetic() || c == '_'));
    if word == "fn" && !named {
        return None;
    }
    Some(word.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(source: &str) -> Vec<(String, &str)> {
        let fixture = Fixture::parse(source).unwrap();
        fixture
            .items
            .iter()
            .map(|item| (item.keyword.clone(), &source[item.start..item.end]))
            .collect()
    }

    #[test]
    fn the_header_ends_at_the_first_line_of_code() {
        let source = "// parity: vm\n\n// expect: success\n/* prose */\nfn main() {}\n";
        assert_eq!(
            header_end(source),
            "// parity: vm\n\n// expect: success\n".len()
        );
    }

    #[test]
    fn items_take_their_comments_and_keep_braceless_ones_whole() {
        let source = "// h\nimport Flow.List as L\n\n/// doc\nfn a() -> Int {\n  1\n}\n\ntype G = A\n  | B\n// tail note\nlet x =\n  a()\npublic fn main() { print(x) }\n";
        assert_eq!(
            pieces(source),
            [
                ("import".to_string(), "import Flow.List as L\n\n"),
                ("fn".to_string(), "/// doc\nfn a() -> Int {\n  1\n}\n\n"),
                ("type".to_string(), "type G = A\n  | B\n"),
                ("let".to_string(), "// tail note\nlet x =\n  a()\n"),
                ("fn".to_string(), "public fn main() { print(x) }\n"),
            ]
        );
    }

    #[test]
    fn a_keyword_inside_a_body_or_a_string_starts_nothing() {
        let source = "fn main() {\nlet s = \"\nfn x() {\"\n}\n@fip fn f(x) { x }\n";
        let keywords: Vec<String> = pieces(source).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keywords, ["fn", "fn"]);
    }

    #[test]
    fn a_multi_line_signature_stays_with_its_item() {
        let source = "fn f(x: Int)\n  -> Int with IO {\n  x\n}\nfn main() {}\n";
        assert_eq!(pieces(source).len(), 2);
    }

    #[test]
    fn code_before_the_first_item_is_an_error() {
        assert!(Fixture::parse("// h\nprint(1)\nfn main() {}\n").is_err());
    }

    #[test]
    fn unbalanced_braces_are_an_error() {
        assert!(Fixture::parse("fn main() {\n").is_err());
        assert!(Fixture::parse("fn main() }\n").is_err());
    }
}
