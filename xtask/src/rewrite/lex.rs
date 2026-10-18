//! A tokenizer for Flux source that is just enough to rewrite fixtures:
//! xtask is std-only, so it cannot use the compiler's lexer. It knows what
//! can hide a brace or a name: comments (`/* */` nests), strings with escapes
//! and `#{…}` interpolation (which can hold further strings), and `"""`
//! strings. Inside an interpolation it yields ordinary tokens, so a rename
//! reaches the names there.

/// What a token is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ident,
    Number,
    /// A piece of a string literal: from its opening quote, or from the `}`
    /// that ends an interpolation, up to its closing quote or the next `#{`.
    Str,
    Comment,
    /// `///` or `/** */`, which belongs to the item after it.
    DocComment,
    Punct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    /// Byte range in the source.
    pub start: usize,
    pub end: usize,
    /// Whether the token is inside a string's `#{…}`.
    pub in_interpolation: bool,
}

impl Token {
    pub fn text(self, source: &str) -> &str {
        &source[self.start..self.end]
    }

    pub fn is(self, source: &str, text: &str) -> bool {
        self.text(source) == text
    }

    /// Whether the token is code, not a comment.
    pub fn is_code(self) -> bool {
        !matches!(self.kind, Kind::Comment | Kind::DocComment)
    }
}

/// Words that are never a variable or a function.
const KEYWORDS: [&str; 26] = [
    "let",
    "fn",
    "if",
    "else",
    "match",
    "with",
    "handle",
    "perform",
    "do",
    "true",
    "false",
    "return",
    "import",
    "public",
    "data",
    "type",
    "effect",
    "alias",
    "class",
    "instance",
    "module",
    "intrinsic",
    "where",
    "in",
    "resume",
    "self",
];

pub fn is_keyword(word: &str) -> bool {
    KEYWORDS.contains(&word)
}

/// Operators lexed as one token, longest first, so `...` is not three field
/// accesses and `->` can be told apart from `-`.
const MULTI: [&str; 15] = [
    "...", "..", "->", "=>", "<-", "==", "!=", "<=", ">=", "&&", "||", "|>", "::", "++", "<>",
];

pub fn tokenize(source: &str) -> Result<Vec<Token>, String> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    // One entry per string whose `#{` we are inside: whether the string is
    // `"""`, and the braces opened since. The `}` that finds the count at
    // zero resumes that string.
    let mut interpolations: Vec<(bool, usize)> = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let c = bytes[at];
        let in_interpolation = !interpolations.is_empty();
        let token = |kind, start, end| Token {
            kind,
            start,
            end,
            in_interpolation,
        };
        if c.is_ascii_whitespace() {
            at += 1;
        } else if source[at..].starts_with("//") {
            let end = source[at..].find('\n').map_or(bytes.len(), |n| at + n);
            let doc = source[at..].starts_with("///") && !source[at..].starts_with("////");
            let kind = if doc { Kind::DocComment } else { Kind::Comment };
            tokens.push(token(kind, at, end));
            at = end;
        } else if source[at..].starts_with("/*") {
            let end = block_comment_end(source, at)?;
            let doc = source[at..].starts_with("/**") && !source[at..].starts_with("/**/");
            let kind = if doc { Kind::DocComment } else { Kind::Comment };
            tokens.push(token(kind, at, end));
            at = end;
        } else if c == b'"' {
            let triple = source[at..].starts_with("\"\"\"");
            let body = at + if triple { 3 } else { 1 };
            let (end, interpolates) = string_part(source, body, triple)?;
            tokens.push(token(Kind::Str, at, end));
            at = end;
            if interpolates {
                interpolations.push((triple, 0));
            }
        } else if c == b'{' {
            if let Some((_, depth)) = interpolations.last_mut() {
                *depth += 1;
            }
            tokens.push(token(Kind::Punct, at, at + 1));
            at += 1;
        } else if c == b'}' && interpolations.last().is_some_and(|(_, depth)| *depth == 0) {
            let (triple, _) = interpolations.pop().expect("checked above");
            let (end, interpolates) = string_part(source, at + 1, triple)?;
            let resumed = Token {
                kind: Kind::Str,
                start: at,
                end,
                in_interpolation: !interpolations.is_empty(),
            };
            tokens.push(resumed);
            at = end;
            if interpolates {
                interpolations.push((triple, 0));
            }
        } else if c == b'}' {
            if let Some((_, depth)) = interpolations.last_mut() {
                *depth -= 1;
            }
            tokens.push(token(Kind::Punct, at, at + 1));
            at += 1;
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let end = scan(bytes, at, |b| b.is_ascii_alphanumeric() || b == b'_');
            tokens.push(token(Kind::Ident, at, end));
            at = end;
        } else if c.is_ascii_digit() {
            let mut end = scan(bytes, at, |b| b.is_ascii_alphanumeric() || b == b'_');
            if bytes.get(end) == Some(&b'.') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit) {
                end = scan(bytes, end + 1, |b| b.is_ascii_alphanumeric() || b == b'_');
            }
            tokens.push(token(Kind::Number, at, end));
            at = end;
        } else {
            let len = MULTI
                .iter()
                .find(|op| source[at..].starts_with(*op))
                .map_or_else(
                    || source[at..].chars().next().map_or(1, char::len_utf8),
                    |op| op.len(),
                );
            tokens.push(token(Kind::Punct, at, at + len));
            at += len;
        }
    }
    if interpolations.is_empty() {
        Ok(tokens)
    } else {
        Err("unterminated string interpolation".to_string())
    }
}

fn scan(bytes: &[u8], from: usize, keep: impl Fn(u8) -> bool) -> usize {
    bytes[from..]
        .iter()
        .position(|&b| !keep(b))
        .map_or(bytes.len(), |n| from + n)
}

/// The end of the `/* */` comment at `start`, counting nested ones.
fn block_comment_end(source: &str, start: usize) -> Result<usize, String> {
    let mut depth = 0;
    let mut at = start;
    while at < source.len() {
        if source[at..].starts_with("/*") {
            depth += 1;
            at += 2;
        } else if source[at..].starts_with("*/") {
            depth -= 1;
            at += 2;
            if depth == 0 {
                return Ok(at);
            }
        } else {
            at += source[at..].chars().next().map_or(1, char::len_utf8);
        }
    }
    Err("unterminated block comment".to_string())
}

/// Scans string text from `at` to its closing quote or to an interpolation's
/// `#{`. Returns where the piece ends and whether it ended at `#{`.
fn string_part(source: &str, mut at: usize, triple: bool) -> Result<(usize, bool), String> {
    let close = if triple { "\"\"\"" } else { "\"" };
    while at < source.len() {
        let rest = &source[at..];
        if let Some(escaped) = rest.strip_prefix('\\') {
            at += 1 + escaped.chars().next().map_or(0, char::len_utf8);
        } else if rest.starts_with("#{") {
            return Ok((at + 2, true));
        } else if rest.starts_with(close) {
            return Ok((at + close.len(), false));
        } else {
            at += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    Err("unterminated string".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(source: &str) -> Vec<&str> {
        tokenize(source)
            .unwrap()
            .into_iter()
            .map(|t| t.text(source))
            .collect()
    }

    #[test]
    fn nested_block_comments_are_one_token() {
        assert_eq!(
            texts("a /* x /* y */ z */ b"),
            ["a", "/* x /* y */ z */", "b"]
        );
    }

    #[test]
    fn a_brace_in_a_string_is_not_a_brace() {
        assert_eq!(texts(r#"f("{ \" }")"#), ["f", "(", r#""{ \" }""#, ")"]);
    }

    #[test]
    fn an_interpolation_yields_its_tokens_and_resumes_the_string() {
        let source = r#""n: #{if ok { "yes" } else { "no" }}!""#;
        assert_eq!(
            texts(source),
            [
                "\"n: #{", "if", "ok", "{", "\"yes\"", "}", "else", "{", "\"no\"", "}", "}!\""
            ]
        );
        let tokens = tokenize(source).unwrap();
        assert!(!tokens[0].in_interpolation);
        assert!(tokens[1].in_interpolation);
        assert!(!tokens.last().unwrap().in_interpolation);
    }

    #[test]
    fn a_triple_quoted_string_spans_lines_and_quotes() {
        assert_eq!(
            texts("x = \"\"\"a \"b\"\nc\"\"\" y"),
            ["x", "=", "\"\"\"a \"b\"\nc\"\"\"", "y"]
        );
    }

    #[test]
    fn doc_comments_are_told_from_comments() {
        let source = "/// doc\n// plain\n/** block doc */ /* plain */";
        let kinds: Vec<Kind> = tokenize(source).unwrap().iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            [
                Kind::DocComment,
                Kind::Comment,
                Kind::DocComment,
                Kind::Comment
            ]
        );
    }

    #[test]
    fn operators_keep_their_length() {
        assert_eq!(
            texts("{ ...a, b: c.d } -> x"),
            [
                "{", "...", "a", ",", "b", ":", "c", ".", "d", "}", "->", "x"
            ]
        );
    }

    #[test]
    fn unterminated_input_is_an_error() {
        assert!(tokenize("\"abc").is_err());
        assert!(tokenize("/* abc").is_err());
        assert!(tokenize("\"#{ a ").is_err());
    }
}
