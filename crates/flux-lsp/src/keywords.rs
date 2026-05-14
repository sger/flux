//! Hover documentation for Flux keywords.
//!
//! Modeled on rust-analyzer's keyword-hover but simplified for our scale:
//! a single static lookup table maps each reserved word to a markdown
//! string with prose plus a code example. The hover handler queries this
//! after rejecting comment/string contexts so prose like `// use let to
//! bind` doesn't trigger keyword docs.

/// Static `(keyword, markdown)` table. Keep alphabetically sorted so growth
/// stays scannable. Search is linear; the list is small enough that a
/// `HashMap` would buy nothing.
const KEYWORD_DOCS: &[(&str, &str)] = &[
    (
        "alias",
        "**`alias`** — Introduce a type or effect-row alias.

Aliases are transparent: at type-checking time they expand to their right-hand \
side. Useful for naming common effect rows.

```flux
alias IO = <Console | FileSystem | Stdin>
alias Pair<a, b> = (a, b)
```",
    ),
    (
        "as",
        "**`as`** — Local alias in an `import` declaration.

```flux
import Flow.Array as Array
```",
    ),
    (
        "class",
        "**`class`** — Declare a type class (Rust trait / Haskell class).

A class names a set of operations indexed by one or more type parameters. \
Implementations are provided by `instance` declarations.

```flux
class Show<a> {
    show: a -> String
}
```",
    ),
    (
        "data",
        "**`data`** — Declare an algebraic data type.

A data declaration introduces a nominal type with one or more named variants. \
Each variant may carry positional fields or named fields.

```flux
data Shape {
    Circle(Float),
    Rectangle { width: Float, height: Float }
}
```",
    ),
    (
        "do",
        "**`do`** — Block expression: sequence multiple statements producing the last value.

Useful where a single expression is required but multiple steps are needed.

```flux
let result = do {
    let a = compute_one()
    let b = compute_two()
    a + b
}
```",
    ),
    (
        "effect",
        "**`effect`** — Declare a user-defined algebraic effect.

Each operation listed in the body becomes a `perform`-able action. Handlers \
discharge the effect by giving an interpretation for every operation.

```flux
effect State {
    get: () -> Int,
    put: Int -> Unit
}
```",
    ),
    (
        "else",
        "**`else`** — Alternative branch of an `if` expression.

`else` may immediately introduce another `if` for chained conditions.

```flux
if x > 0 { \"+\" } else if x < 0 { \"-\" } else { \"0\" }
```",
    ),
    (
        "exposing",
        "**`exposing`** — Specify which members of an `import` enter unqualified scope.

```flux
import Flow.String exposing (join, split)
import Flow.Numeric exposing (..)
```",
    ),
    (
        "false",
        "**Boolean literal.**

`true` and `false` are the two values of type `Bool`.",
    ),
    (
        "fn",
        "**`fn`** — Declare a function.

Parameter and return types are inferred unless annotated. Effects performed by \
the body appear in a trailing `with` clause.

```flux
fn add(x: Int, y: Int) -> Int { x + y }
fn greet(name: String) with IO { print(\"Hello, \" + name) }
```",
    ),
    (
        "handle",
        "**`handle`** — Run a computation under a handler for one or more effects.

Each arm specifies how an effect operation is interpreted; the `return` arm \
maps the final value. Effects handled here are discharged from the result row.

```flux
handle counter() with {
    get() -> resume(0),
    put(_) -> resume(()),
}
```",
    ),
    (
        "if",
        "**`if`** — Conditional expression.

`if` is an expression: both branches must produce the same type. The `else` \
arm is required when `if` is used as a value.

```flux
let label = if x > 0 { \"positive\" } else { \"non-positive\" }
```",
    ),
    (
        "import",
        "**`import`** — Bring another module's members into scope.

Use `exposing (..)` to import unqualified, `exposing (a, b)` for selective \
imports, or `as Name` to introduce a local alias.

```flux
import Flow.Array as Array
import Flow.String exposing (join, split)
```",
    ),
    (
        "instance",
        "**`instance`** — Provide a `class` implementation for a specific type.

The compiler dispatches class methods through the matching instance at use sites.

```flux
instance Show<Int> {
    show(x) { Int.to_string(x) }
}
```",
    ),
    (
        "intrinsic",
        "**`intrinsic`** — Bind a name to a compiler primop.

Used inside `Flow.Primops` to expose compiler-level operations as ordinary \
functions. Application code rarely writes this directly.

```flux
intrinsic fn print<a>(x: a) -> Unit with Console = primop Print
```",
    ),
    (
        "let",
        "**`let`** — Bind a value to a name.

The value's type is inferred unless annotated. Bindings are immutable; rebind \
with a fresh `let` to shadow.

```flux
let answer = 42
let name: String = \"Flux\"
```",
    ),
    (
        "match",
        "**`match`** — Pattern-match on a value.

The compiler checks all arms cover the scrutinee's shape (exhaustiveness). \
Use `_` for the wildcard arm.

```flux
match shape {
    Circle(r) -> Float.pi * r * r,
    Rectangle { width, height } -> width * height
}
```",
    ),
    (
        "module",
        "**`module`** — Group declarations under a qualified name.

All members declared inside a `module Foo { ... }` block are accessed as \
`Foo.member`. A file may declare one top-level module to mirror its filename.

```flux
module Geometry {
    public fn area(s: Shape) -> Float { ... }
}
```",
    ),
    (
        "perform",
        "**`perform`** — Invoke an effect operation.

The enclosing function's effect row must include this effect, or a `handle` \
block must discharge it.

```flux
fn read_state() -> Int with State { perform get() }
```",
    ),
    (
        "public",
        "**`public`** — Export a declaration from its enclosing module.

Without `public`, declarations are visible only inside the module that defines \
them. Applies to `data`, `fn`, `let`, `class`, `instance`, and `alias`.

```flux
public data Point { Point { x: Float, y: Float } }
public fn origin() -> Point { Point { x: 0.0, y: 0.0 } }
```",
    ),
    (
        "return",
        "**`return`** — Return a value early from a function.

Most Flux functions return their last expression implicitly; `return` is for \
early exits.

```flux
fn first_positive(xs) {
    for x in xs { if x > 0 { return x } }
    -1
}
```",
    ),
    (
        "true",
        "**Boolean literal.**

`true` and `false` are the two values of type `Bool`.",
    ),
    (
        "with",
        "**`with`** — Effect annotation.

Following `with` is the effect row a function or block produces. Multiple \
effects are written `with E1, E2` or via an alias like `with IO`.

```flux
fn main() with IO { print(\"hi\") }
```",
    ),
];

/// Return the hover markdown for a reserved word, or `None` if `word` is not
/// a recognized Flux keyword.
pub fn keyword_doc(word: &str) -> Option<&'static str> {
    KEYWORD_DOCS
        .iter()
        .find(|(k, _)| *k == word)
        .map(|(_, doc)| *doc)
}

/// Extract the identifier-shaped word covering byte offset `off` from
/// `text`, or `None` if `off` is on whitespace / punctuation. Used by the
/// hover handler to decide whether to surface keyword documentation
/// before delegating to the locator.
pub fn word_at_offset(text: &str, off: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    if off > bytes.len() {
        return None;
    }
    let pivot = if off < bytes.len() && is_ident_byte(bytes[off]) {
        off
    } else if off > 0 && is_ident_byte(bytes[off - 1]) {
        off - 1
    } else {
        return None;
    };
    let mut start = pivot;
    while start > 0 && is_ident_byte(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = pivot;
    while end < bytes.len() && is_ident_byte(bytes[end]) {
        end += 1;
    }
    std::str::from_utf8(&bytes[start..end]).ok()
}

/// True if byte offset `off` falls inside a `//` line comment, a `/* */`
/// block comment, a `"..."` string literal, or a `///` doc comment.
///
/// Implemented as a linear state-machine scan from the start of the buffer.
/// Cheap for the buffer sizes the LSP handles (a few KB), and avoids a
/// dependency on the lexer's internal token list (the lexer doesn't emit
/// tokens for plain `//` / `/* */` comments, so we couldn't use it
/// directly anyway).
pub fn is_offset_in_comment_or_string(text: &str, off: usize) -> bool {
    let bytes = text.as_bytes();
    let limit = off.min(bytes.len());
    let mut i = 0;
    let mut state = ScanState::Code;
    while i < limit {
        let b = bytes[i];
        let next = if i + 1 < bytes.len() {
            Some(bytes[i + 1])
        } else {
            None
        };
        match state {
            ScanState::Code => match (b, next) {
                (b'/', Some(b'/')) => {
                    state = ScanState::LineComment;
                    i += 2;
                }
                (b'/', Some(b'*')) => {
                    state = ScanState::BlockComment(1);
                    i += 2;
                }
                (b'"', _) => {
                    state = ScanState::StringLit;
                    i += 1;
                }
                _ => i += 1,
            },
            ScanState::LineComment => {
                if b == b'\n' {
                    state = ScanState::Code;
                }
                i += 1;
            }
            ScanState::BlockComment(depth) => match (b, next) {
                (b'*', Some(b'/')) => {
                    let new_depth = depth - 1;
                    state = if new_depth == 0 {
                        ScanState::Code
                    } else {
                        ScanState::BlockComment(new_depth)
                    };
                    i += 2;
                }
                (b'/', Some(b'*')) => {
                    state = ScanState::BlockComment(depth + 1);
                    i += 2;
                }
                _ => i += 1,
            },
            ScanState::StringLit => {
                if b == b'\\' && next.is_some() {
                    i += 2;
                } else if b == b'"' {
                    state = ScanState::Code;
                    i += 1;
                } else {
                    i += 1;
                }
            }
        }
    }
    !matches!(state, ScanState::Code)
}

#[derive(Clone, Copy)]
enum ScanState {
    Code,
    LineComment,
    BlockComment(u32),
    StringLit,
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_keywords_have_docs() {
        assert!(keyword_doc("let").is_some());
        assert!(keyword_doc("fn").is_some());
        assert!(keyword_doc("data").is_some());
        assert!(keyword_doc("public").is_some());
        assert!(keyword_doc("with").is_some());
    }

    #[test]
    fn unknown_word_returns_none() {
        assert!(keyword_doc("Person").is_none());
        assert!(keyword_doc("xyz").is_none());
    }

    #[test]
    fn word_at_offset_finds_word() {
        assert_eq!(word_at_offset("let x = 1", 0), Some("let"));
        assert_eq!(word_at_offset("let x = 1", 2), Some("let"));
        assert_eq!(word_at_offset("let x = 1", 3), Some("let"));
        assert_eq!(word_at_offset("let x = 1", 4), Some("x"));
        assert_eq!(word_at_offset("let x = 1", 6), None);
    }

    #[test]
    fn detects_line_comment_context() {
        // Offset of `let` inside `// use let to bind`.
        let src = "// use let to bind\nlet x = 1\n";
        let off = src.find("let").unwrap();
        assert!(is_offset_in_comment_or_string(src, off));
        // Offset of `let` on line 2 — outside comment.
        let off2 = src.rfind("let").unwrap();
        assert!(!is_offset_in_comment_or_string(src, off2));
    }

    #[test]
    fn detects_block_comment_context() {
        let src = "/* fn add() */ let x = 1\n";
        let fn_off = src.find("fn").unwrap();
        assert!(is_offset_in_comment_or_string(src, fn_off));
        let let_off = src.find("let").unwrap();
        assert!(!is_offset_in_comment_or_string(src, let_off));
    }

    #[test]
    fn detects_string_context() {
        let src = "let m = \"the let keyword\"\n";
        // First `let` is the binder; second is inside the string.
        let first_let = src.find("let").unwrap();
        let second_let = src.rfind("let").unwrap();
        assert!(!is_offset_in_comment_or_string(src, first_let));
        assert!(is_offset_in_comment_or_string(src, second_let));
    }

    #[test]
    fn nested_block_comments_are_tracked() {
        let src = "/* outer /* inner */ still in outer */ fn x() {}\n";
        let fn_off = src.find("fn").unwrap();
        assert!(!is_offset_in_comment_or_string(src, fn_off));
        let inner_off = src.find("inner").unwrap();
        assert!(is_offset_in_comment_or_string(src, inner_off));
    }
}
