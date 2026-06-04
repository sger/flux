//! Hover documentation for Flux keywords and built-in effects.
//!
//! Modeled on rust-analyzer's keyword-hover but simplified for our scale:
//! a single static lookup table maps each reserved word to a markdown
//! string with prose plus a code example. The hover handler queries this
//! after rejecting comment/string contexts so prose like `// use let to
//! bind` doesn't trigger keyword docs.
//!
//! Two sources contribute keywords:
//! - **Lexer-reserved words** are listed in
//!   `flux::syntax::token_type::KEYWORDS`. A drift test in this file
//!   asserts every entry there has matching hover content (except the
//!   built-in ADT constructors `Some`/`None`/`Left`/`Right`, whose hover
//!   comes from the AST path showing the inferred type). A new lexer
//!   keyword fails CI until a doc lands here.
//! - **Contextual keywords** (`exposing`, `except`, `end`, `ambient`,
//!   `resume`) are recognized only in specific syntactic positions; they
//!   parse as ordinary identifiers elsewhere. The drift test also covers
//!   them.

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
        "ambient",
        "**`ambient`** — Reference the enclosing effect row inside a `sealing` clause.

In the algebraic form `expr sealing (ambient - E1 - E2)`, `ambient` stands for \
the effect row the surrounding scope provides; the subtraction restricts \
`expr` to a strict subset of it.

```flux
fn inner() with Console, FileSystem { ... } sealing (ambient - FileSystem)
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
        "**`class`** — Declare a type class.

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
        "deriving",
        "**`deriving`** — Auto-generate type-class instances for a `data` declaration.

The compiler synthesizes the listed classes' methods based on the data shape. \
Common candidates are `Eq` and `Show`.

```flux
data Color { Red, Green, Blue } deriving (Eq, Show)
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
        "end",
        "**`end`** — Optional terminator that closes a `module` block.

Most blocks close with `}`. `end` is accepted as a soft terminator in \
positions where matching the opening keyword reads more clearly than a brace.

```flux
module Geometry
    public fn area(s: Shape) -> Float { ... }
end
```",
    ),
    (
        "except",
        "**`except`** — Exclude members from an `exposing (..)` clause.

Pair with `exposing (..)` to expose everything except a few names — useful \
when only one or two identifiers conflict.

```flux
import Flow.Math exposing (..) except (sqrt, pow)
```",
    ),
    (
        "exposing",
        "**`exposing`** — Specify which members of an `import` enter unqualified scope.

List names explicitly, or use `(..)` for every public member. Without an \
`exposing` clause, members stay qualified under the module name.

```flux
import Flow.String exposing (join, split)
import Flow.Numeric exposing (..)
```",
    ),
    (
        "false",
        "**`false`** — Boolean literal.

`true` and `false` are the two values of type `Bool`, produced by comparison \
and logical operators.

```flux
let done = false
```",
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
        "primop",
        "**`primop`** — Bind an `intrinsic` function to a named compiler primitive.

Appears on the right-hand side of `intrinsic fn ... = primop X`. The compiler \
expands the call site to the named CorePrimOp. End-user code does not write \
`primop` directly; it shows up only inside `Flow.Primops`.

```flux
intrinsic fn print<a>(x: a) -> Unit with Console = primop Print
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
        "resume",
        "**`resume`** — Continue an effect handler with a value.

Inside a `handle` arm, `resume(v)` returns control to the computation that \
performed the effect, supplying `v` as the result of the `perform` call. \
Omitting `resume` aborts the handled computation entirely.

```flux
handle counter() with {
    get() -> resume(0),
    put(_) -> resume(()),
}
```",
    ),
    (
        "sealing",
        "**`sealing`** — Restrict the effect row of an expression.

`expr sealing { E1, E2 }` constrains the contained expression to perform at \
most the listed effects; effects outside that set become a type error inside \
the sealed scope.

```flux
console_only() sealing { Console }
```",
    ),
    (
        "select",
        "**`select`** — Wait on multiple async operations, returning the first ready.

Each arm names a channel `recv`/`send` or a timer; the block evaluates the \
arm whose source fires first.

```flux
select {
    recv ch as value -> use_value(value),
    after 100 -> \"timeout\",
}
```",
    ),
    (
        "true",
        "**`true`** — Boolean literal.

`true` and `false` are the two values of type `Bool`, produced by comparison \
and logical operators.

```flux
let ready = true
```",
    ),
    (
        "type",
        "**`type`** — Declare a transparent type alias (synonym for `alias`).

The right-hand side may be any type expression. At type-checking time the \
alias name expands to its body.

```flux
type Shape = Circle(Float) | Rect(Float, Float)
```",
    ),
    (
        "where",
        "**`where`** — Introduce a let-binding scoped to the preceding clause.

Common in list-comprehension-style code: `expr where x = ...` makes `x` \
available throughout `expr`. Multiple `where` clauses may chain.

```flux
let result = process(left, right)
    where left  = data |> Array.map(fst)
    where right = data |> Array.map(snd)
```",
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

/// Static `(effect, markdown)` table for built-in effect labels — the
/// aliases (`IO`, `Time`, `Async`), the fine-grained labels (`Console`,
/// `FileSystem`, …), and the async seams. Same card shape as
/// [`KEYWORD_DOCS`]; descriptions track `flux::syntax::builtin_effects`.
/// Keep alphabetically sorted.
const EFFECT_DOCS: &[(&str, &str)] = &[
    (
        "Async",
        "**`Async`** — Effect alias for asynchronous computation.

`Async` expands to `<Suspend | Fork | GetContext | AsyncFail>`. Annotate code \
that awaits I/O, forks fibers, or can fail asynchronously with `with Async`.

```flux
fn fetch(url: String) -> String with Async { ... }
```",
    ),
    (
        "AsyncFail",
        "**`AsyncFail`** — Asynchronous-failure seam.

Raised by `Async.fail` and by `timeout` expiry. Part of the `Async` alias; \
user code writes `with Async` rather than `with AsyncFail`.

```flux
fn racy() -> Int with Async { ... }
```",
    ),
    (
        "Clock",
        "**`Clock`** — Wall-clock and monotonic-time effect.

Carried by `clock_now` and `now_ms`. The fine-grained label behind the \
`Time` alias.

```flux
fn stamp() -> Int with Clock { now_ms() }
```",
    ),
    (
        "Console",
        "**`Console`** — Standard-output effect.

Carried by `print` and `println`. A function that writes to stdout lists \
`Console` — or the broader `IO` alias — in its `with` clause.

```flux
fn greet(name: String) with Console { print(\"Hi, \" + name) }
```",
    ),
    (
        "Debug",
        "**`Debug`** — Developer-tracing effect.

Carried by `debug`, `debug_labeled`, and `debug_with` — diagnostic output to \
stderr. Separate from `Console` so traces can be captured or silenced \
independently of program output.

```flux
fn step(x: Int) -> Int with Debug { debug(x); x + 1 }
```",
    ),
    (
        "Div",
        "**`Div`** — Recoverable-failure effect.

Carried by integer division and modulo and by indexing — operations that can \
fail at runtime on division by zero or an out-of-bounds access.

```flux
fn half(x: Int) -> Int with Div { x / 2 }
```",
    ),
    (
        "Exn",
        "**`Exn`** — Recoverable-exception effect (reserved).

Documented in `Flow.Effects` for future use; not yet emitted operationally \
by compiler primops.

```flux
fn risky() -> Int with Exn { ... }
```",
    ),
    (
        "FileSystem",
        "**`FileSystem`** — Filesystem input/output effect.

Carried by `read_file`, `write_file`, and `read_lines`.

```flux
fn load(path: String) -> String with FileSystem { read_file(path) }
```",
    ),
    (
        "Fork",
        "**`Fork`** — Fiber-fork seam.

Emitted by structured-concurrency fork operations. Part of the `Async` \
alias; user code writes `with Async`.

```flux
fn spawn_work() with Async { ... }
```",
    ),
    (
        "GetContext",
        "**`GetContext`** — Fiber-context seam.

Used to retrieve the current `FiberContext`. Part of the `Async` alias; \
user code writes `with Async`.

```flux
fn current() with Async { ... }
```",
    ),
    (
        "IO",
        "**`IO`** — Effect alias for general input/output.

`IO` expands to `<Console | FileSystem | Stdin>` — a function `with IO` may \
print, read files, and read stdin. Prefer a fine-grained label when only one \
channel is needed.

```flux
fn main() with IO { print(\"hi\") }
```",
    ),
    (
        "NonDet",
        "**`NonDet`** — Non-determinism effect (reserved).

Documented in `Flow.Effects` for future use; not yet emitted operationally \
by compiler primops.

```flux
fn choose() -> Int with NonDet { ... }
```",
    ),
    (
        "Panic",
        "**`Panic`** — Intentional-crash effect.

Carried by `panic`. Kept separate from `Exn` because a panic cannot be \
discarded by the optimizer — it always aborts the program.

```flux
fn unreachable() with Panic { panic(\"impossible\") }
```",
    ),
    (
        "Random",
        "**`Random`** — Randomness effect (reserved).

Documented in `Flow.Effects` for future use; not yet emitted operationally \
by compiler primops.

```flux
fn roll() -> Int with Random { ... }
```",
    ),
    (
        "Stdin",
        "**`Stdin`** — Standard-input effect.

Carried by `read_stdin` — reading a line from standard input.

```flux
fn ask() -> String with Stdin { read_stdin() }
```",
    ),
    (
        "Suspend",
        "**`Suspend`** — Fiber-suspension seam.

Emitted by the compiler at I/O await points. Part of the `Async` alias; user \
code writes `with Async`, never `with Suspend` directly.

```flux
fn await_io() with Async { ... }
```",
    ),
    (
        "Time",
        "**`Time`** — Effect alias for clock access.

`Time` expands to `<Clock>` — a function `with Time` may read the wall-clock \
or monotonic time.

```flux
fn stamp() -> Int with Time { now_ms() }
```",
    ),
];

/// Return the hover markdown for a built-in effect label, or `None` for a
/// name that is not a recognized built-in effect (e.g. a user-declared
/// `effect`).
pub fn effect_doc(name: &str) -> Option<&'static str> {
    EFFECT_DOCS
        .iter()
        .find(|(e, _)| *e == name)
        .map(|(_, doc)| *doc)
}

/// Static `(type, markdown)` table for the universal built-in types — the
/// primitives plus the always-in-scope type constructors. Same card shape as
/// [`KEYWORD_DOCS`]. `Result` is intentionally absent: it is a `Flow.Async`
/// module type (`public data Result<a, e>`), not a universal built-in, so it
/// resolves to no doc rather than a misleading one. Keep alphabetically
/// sorted.
const TYPE_DOCS: &[(&str, &str)] = &[
    (
        "Array",
        "**`Array`** — Contiguous, indexed sequence `Array<a>`.

Backed by a packed buffer, so element access is O(1). Prefer over `List` when \
random access or bulk iteration dominates.

```flux
let xs: Array<Int> = [1, 2, 3]
```",
    ),
    (
        "Bool",
        "**`Bool`** — Boolean truth value.

The two values `true` and `false`, produced by comparison and logical \
operators and consumed by `if` and guards.

```flux
let ready: Bool = 1 < 2
```",
    ),
    (
        "Float",
        "**`Float`** — 64-bit IEEE-754 floating-point number.

Float literals carry a decimal point. Mixing `Int` and `Float` requires an \
explicit conversion.

```flux
let pi: Float = 3.14159
```",
    ),
    (
        "Int",
        "**`Int`** — 64-bit signed integer.

The default numeric type. Integer division and modulo carry the `Div` effect \
because they can fail on a zero divisor.

```flux
let count: Int = 42
```",
    ),
    (
        "List",
        "**`List`** — Immutable singly-linked list `List<a>`.

Built from cons cells, so prepend is O(1) and indexed access is O(n). The \
natural shape for head/tail recursion and pattern matching.

```flux
let xs: List<Int> = [1 | [2 | [3 | []]]]
```",
    ),
    (
        "Option",
        "**`Option`** — An optional value `Option<a>`.

Either `Some(x)` carrying a value or `None` for absence. Pattern-match to \
handle both cases without null.

```flux
let found: Option<Int> = Some(7)
```",
    ),
    (
        "String",
        "**`String`** — UTF-8 text.

String literals use double quotes and support interpolation. `Flow.String` \
provides operations like `join`, `split`, and `length`.

```flux
let greeting: String = \"Hello, Flux\"
```",
    ),
];

/// Return the hover markdown for a universal built-in type, or `None` for a
/// name that is not one (a user `data`/`alias`, or a module type like
/// `Result`).
pub fn builtin_type_doc(name: &str) -> Option<&'static str> {
    TYPE_DOCS
        .iter()
        .find(|(t, _)| *t == name)
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

    /// Built-in ADT constructors that are tokenized as keywords by the
    /// lexer (`Some`, `None`, `Left`, `Right`) but whose hover content
    /// comes from the AST path (inferred type, e.g. `Option<Int>`)
    /// rather than a static doc. The drift test skips these — adding a
    /// doc here would shadow the useful inferred-type hover.
    const CONSTRUCTOR_EXCLUSIONS: &[&str] = &["Some", "None", "Left", "Right"];

    /// Contextual keywords: identifiers the parser treats specially in
    /// specific positions (after `import`, inside `sealing { ... }`,
    /// inside a `handle` arm) but that aren't lexer-reserved. Each must
    /// have a hover entry; the drift test enforces this.
    const CONTEXTUAL_KEYWORDS: &[&str] = &["ambient", "end", "except", "exposing", "resume"];

    #[test]
    fn known_keywords_have_docs() {
        assert!(keyword_doc("let").is_some());
        assert!(keyword_doc("fn").is_some());
        assert!(keyword_doc("data").is_some());
        assert!(keyword_doc("public").is_some());
        assert!(keyword_doc("with").is_some());
    }

    /// Every keyword doc follows one uniform shape: a bold
    /// ``**`kw`** — summary.`` header, then prose, then a ```flux fenced
    /// example. This guards against a new entry drifting from the template.
    #[test]
    fn keyword_docs_share_a_uniform_shape() {
        for &(kw, doc) in KEYWORD_DOCS {
            assert!(
                doc.starts_with(&format!("**`{kw}`** — ")),
                "`{kw}` doc must open with a `**`{kw}`** — ` header, got: {doc}"
            );
            assert!(
                doc.contains("```flux\n") && doc.contains("\n```"),
                "`{kw}` doc must include a ```flux fenced example"
            );
        }
    }

    /// Built-in effect docs share the same uniform card shape as keyword
    /// docs — and `effect_doc` resolves a known label while rejecting an
    /// unknown one.
    #[test]
    fn effect_docs_share_a_uniform_shape() {
        for &(effect, doc) in EFFECT_DOCS {
            assert!(
                doc.starts_with(&format!("**`{effect}`** — ")),
                "`{effect}` doc must open with a `**`{effect}`** — ` header, got: {doc}"
            );
            assert!(
                doc.contains("```flux\n") && doc.contains("\n```"),
                "`{effect}` doc must include a ```flux fenced example"
            );
        }
        assert!(effect_doc("IO").is_some());
        assert!(effect_doc("Console").is_some());
        assert!(effect_doc("State").is_none());
    }

    /// Built-in type docs share the same uniform card shape — and
    /// `builtin_type_doc` resolves the universal built-ins while rejecting a
    /// module type (`Result`) or a user type.
    #[test]
    fn type_docs_share_a_uniform_shape() {
        for &(ty, doc) in TYPE_DOCS {
            assert!(
                doc.starts_with(&format!("**`{ty}`** — ")),
                "`{ty}` doc must open with a `**`{ty}`** — ` header, got: {doc}"
            );
            assert!(
                doc.contains("```flux\n") && doc.contains("\n```"),
                "`{ty}` doc must include a ```flux fenced example"
            );
        }
        assert!(builtin_type_doc("Int").is_some());
        assert!(builtin_type_doc("Option").is_some());
        assert!(builtin_type_doc("Result").is_none());
        assert!(builtin_type_doc("Person").is_none());
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

    /// Drift gate: every lexer keyword in `flux::syntax::token_type::KEYWORDS`
    /// must have a hover entry here, *except* the built-in ADT constructors
    /// listed in [`CONSTRUCTOR_EXCLUSIONS`] (whose hover comes from the
    /// inferred type via the AST path). Adding a new keyword to the lexer
    /// without a doc — or accidentally adding a doc for an excluded
    /// constructor — fails this test.
    #[test]
    fn every_lexer_keyword_has_hover_doc() {
        use flux::syntax::token_type::KEYWORDS;

        let mut missing: Vec<&str> = Vec::new();
        let mut unexpected: Vec<&str> = Vec::new();
        for kw in KEYWORDS {
            let excluded = CONSTRUCTOR_EXCLUSIONS.contains(kw);
            let has_doc = keyword_doc(kw).is_some();
            match (excluded, has_doc) {
                (false, false) => missing.push(*kw),
                (true, true) => unexpected.push(*kw),
                _ => {}
            }
        }
        assert!(
            missing.is_empty(),
            "lexer keywords lacking hover docs: {missing:?}. Add entries to KEYWORD_DOCS."
        );
        assert!(
            unexpected.is_empty(),
            "constructor exclusions unexpectedly have hover docs: {unexpected:?}. \
             Either remove the doc or update CONSTRUCTOR_EXCLUSIONS."
        );
    }

    /// Drift gate for contextual keywords (`exposing`, `except`, `end`,
    /// `ambient`, `resume`). These aren't lexer-reserved, so they're listed
    /// by hand in [`CONTEXTUAL_KEYWORDS`]; the test asserts each has hover
    /// content.
    #[test]
    fn every_contextual_keyword_has_hover_doc() {
        let missing: Vec<&str> = CONTEXTUAL_KEYWORDS
            .iter()
            .copied()
            .filter(|kw| keyword_doc(kw).is_none())
            .collect();
        assert!(
            missing.is_empty(),
            "contextual keywords lacking hover docs: {missing:?}"
        );
    }

    /// Catches the inverse: a keyword doc entry exists for a word that is
    /// neither a lexer keyword nor a contextual keyword. Likely a typo or
    /// a renamed keyword whose old entry was left behind.
    #[test]
    fn no_orphan_keyword_docs() {
        use flux::syntax::token_type::KEYWORDS;

        let orphans: Vec<&str> = KEYWORD_DOCS
            .iter()
            .map(|(kw, _)| *kw)
            .filter(|kw| !KEYWORDS.contains(kw) && !CONTEXTUAL_KEYWORDS.contains(kw))
            .collect();
        assert!(
            orphans.is_empty(),
            "KEYWORD_DOCS entries match no known keyword: {orphans:?}"
        );
    }
}
