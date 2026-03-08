- Feature Name: Quality Diagnostics
- Start Date: 2026-03-07
- Status: Not Implemented
- Proposal PR:
- Flux Issue:
- Depends on: 0059 (parser error experience), 0061 (stage-aware diagnostic pipeline)

# Proposal 0080: Quality Diagnostics

## Summary

Rewrite every compiler diagnostic to meet the Elm standard: natural first-person
language, clearly separated "I found / I expected" type blocks, pattern-based
fix suggestions, and zero jargon leakage to the user.

## Motivation

Elm's compiler is the industry benchmark for diagnostic quality. Every error has
three parts: *what the compiler found*, *what it expected*, and *what to do
about it*. The developer never sees internal representation, never reads
"Cannot unify X with Y", and always gets an actionable hint.

The compiler vision for Flux is to inform the developer for every error —
not just "something went wrong" but *what*, *where in the pipeline*, and
*what to do about it*. The pipeline maps error domains to code ranges:

```
Source → Lexer → Parser → HM Inference → Bytecode Compiler → VM/JIT
  E001-E010  E034+  E300-E302    E400-E425       E1000+
```

### Current state by layer

**Lexer** — mostly good. Unterminated strings, invalid chars, and bad floats all
have dedicated messages. Minor gap: no recovery hint for `\n` inside a string
literal.

**Parser** — the biggest gap. 54 contextual `expect_peek_context` calls are
solid, but several issues remain:

- `peek_error()` fallback still fires with bare "Expected `X`, got `Y`" — no hint
- Spans point to the *bad token*, never to the *construct that started* the problem
- Error cascades: one missing `{` can produce 4+ follow-on errors
- No "did you mean" for mistyped identifiers (`prnit`, `retun`)

**HM inference** — improved by 0079, but:

- `E300 TYPE UNIFICATION ERROR` still says "Cannot unify X with Y" — doesn't
  explain *why* those types arose or which call caused the constraint
- Occurs check failure (E301) has no actionable hint
- No inference trace: "this variable was inferred as `Int` here, but used as
  `String` here"

**Bytecode compiler** — best coverage today. `E400`/`E413`/`E414`/`E425` all
have specific messages and hints. Weak spots: constraint violations
(`E419`/`E420`/`E422`) list symbols but don't explain the constraint chain.

**Runtime** — `E1000+` are decent but bare: "type error at position X" without
showing the value that failed.

### Specific parser gaps

1. **Cascade quality** — when a `{` or `(` is missing, the parser syncs at the
   wrong boundary and the 2nd/3rd errors are often noise. The current snapshots
   sometimes show 3–4 errors for a single mistake. No existing proposal addresses
   this specifically.

2. **Span precision** — errors point to the *next wrong token*, never to the
   *start of the construct* that failed. For a missing `->` in a long function
   signature, the error lands mid-way through rather than on the `fn` keyword.

3. **No identifier fuzzy matching** — `prnit` gets "Expected expression", not
   "did you mean `print`?". Proposal 0059 (keyword aliases) is Partially
   Implemented — foreign keywords like `var`/`class` may have hints but
   misspelled identifiers do not.

4. **Static structural messages** — messages read "Function declarations start
   with `fn name(...) { ... }`" which is helpful but fixed. If the function is
   inside a module or is a lambda, the message is identical.

5. **`peek_error()` fallback** — the generic "Expected `X`, got `Y`" path is
   still reachable with no hint. It fires from `expect_peek()` which is used
   internally in `helpers.rs`.

6. **No context breadcrumbs** — there is no "while parsing function body at
   line 12" context. The `expect_peek_contextf` variant can query parser state
   but has no call stack.

### Gap → fix matrix

| Layer | Gap | Fix |
|---|---|---|
| Parser | `peek_error()` bare fallback | Convert all remaining bare `expect_peek` to contextual |
| Parser | Cascade noise | Track parse depth; suppress follow-on errors within same construct |
| Parser | Span quality | Record construct-start position as secondary label |
| Parser | No fuzzy matching | Levenshtein distance on undefined identifiers |
| HM | E300 origin trace | Record which expression forced each type variable binding |
| HM | E301 hint | "This type contains itself — define an ADT wrapper" |
| Compiler | E419/E420/E422 chain | Show which call produced the constraint |
| Runtime | E1004 value context | Show the actual value alongside the expected type |

Flux's current diagnostics otherwise fall into three tiers:

| Tier | Examples | State |
|---|---|---|
| Good — contextual, specific | `if_branch_type_mismatch`, `call_arg_type_mismatch`, `E400` (after 0079) | Keep, minor polish |
| Mediocre — structural but repetitive | `fun_return_type_mismatch`, `fun_param_type_mismatch`, `fun_arity_mismatch` | Rewrite |
| Bad — jargon, no hint | `type_unification_error`, `occurs_check_failure`, parser `peek_error()` | Rewrite |

## Guide-level explanation

### The Elm principle applied to Flux

Every diagnostic must answer three questions:

1. **What did I find?** — one sentence, natural language, first person optional.
2. **What did I expect?** — the type or construct shown on its own line, not
   embedded in the sentence.
3. **What should the developer do?** — a concrete, specific `Hint:` line.

### Before / After

#### `type_unification_error` (plain fallback)

**Before:**
```
error[E300]: TYPE UNIFICATION ERROR
Cannot unify Int with String.
  --> file.flx:5:18
   |
5  | let x: String = 42
   |                 ^^ expected String, found Int
```

**After:**
```
error[E300]: TYPE MISMATCH
I found a type mismatch.
  --> file.flx:5:18
   |
5  | let x: String = 42
   |                 ^^ this expression has type `Int`

   = expected type: String
   = found type:    Int

Hint: These two types are not compatible. Check the expression at this location.
```

#### `fun_return_type_mismatch`

**Before:**
```
error[E300]: TYPE UNIFICATION ERROR
Function return types do not match: expected `Int`, found `String`.
  --> file.flx:3:5
   |
3  |     to_string(x)
   |     ^^^^^^^^^^^^ expected return type `Int`, found `String`
```

**After:**
```
error[E300]: TYPE MISMATCH
The body of this function does not match its return type.
  --> file.flx:3:5
   |
3  |     to_string(x)
   |     ^^^^^^^^^^^^ this expression has type `String`

   = declared return type: Int
   = body type:            String

Hint: Change the return annotation to `-> String`, or change the body to return `Int`.
```

#### `fun_arity_mismatch`

**Before:**
```
error[E300]: TYPE UNIFICATION ERROR
Function arity does not match.
  --> file.flx:7:5
   |
7  |     foo(1, 2)
   |     ^^^^^^^^^ expected 1 parameters, found 2
```

**After:**
```
error[E300]: TYPE MISMATCH
I am applying a function to the wrong number of arguments.
  --> file.flx:7:5
   |
7  |     foo(1, 2)
   |     ^^^^^^^^^ this call passes 2 arguments

   = this function takes: 1 argument
   = but this call passes: 2 arguments

Hint: Remove the extra argument.
```

#### `occurs_check_failure`

**Before:**
```
error[E301]: OCCURS CHECK FAILURE
Infinite type: type variable t1 occurs in List t1.
  --> file.flx:4:3
```

**After:**
```
error[E301]: INFINITE TYPE
I found a type that would be infinitely recursive.
  --> file.flx:4:3
   |
4  |     let xs = [xs]
   |     ^^^^^^^^^^^^^ this expression causes an infinite type

Hint: A value cannot contain itself. If you need recursion over a type,
      define an ADT: `type Tree = Leaf | Node(Tree, Tree)`.
```

#### Parser `peek_error()` fallback

**Before:**
```
error[E034]: UNEXPECTED TOKEN
Expected `{`, got `let`.
  --> file.flx:4:12
```

**After:**
```
error[E034]: UNEXPECTED TOKEN
I was expecting a `{` to start the function body here.
  --> file.flx:4:12
   |
4  | fn greet(name) let x = 1
   |                ^^^ I found `let` instead

Hint: All function bodies must be enclosed in braces: `fn greet(name) { ... }`.
```

### Common type-pair hints

The hint text should be pattern-matched against the (expected, actual) pair
and produce a specific suggestion where possible:

| expected | actual | Hint |
|---|---|---|
| `String` | `Int` | "Try `to_string(x)` to convert an `Int` to a `String`." |
| `String` | `Float` | "Try `to_string(x)` to convert a `Float` to a `String`." |
| `Int` | `Float` | "Try `to_int(x)` to truncate a `Float` to an `Int`." |
| `Float` | `Int` | "Try `to_float(x)` to widen an `Int` to a `Float`." |
| `Bool` | `Int` | "Booleans are not integers in Flux. Use `true` or `false`." |
| `T` | `Option<T>` | "This value might be `None`. Use a `match` to unwrap it." |
| `Option<T>` | `T` | "Wrap this in `Some(...)` or return `None`." |

## Reference-level explanation

### Fix 1: `type_unification_error`

Replace the message with first-person natural language. Move both type
strings out of the message sentence and into note lines so they render
as clearly separated blocks:

```rust
pub fn type_unification_error(
    file: impl Into<Rc<str>>,
    span: Span,
    expected: &str,
    actual: &str,
) -> Diagnostic {
    diagnostic_for(&TYPE_UNIFICATION_ERROR)
        .with_file(file)
        .with_span(span)
        .with_message("I found a type mismatch.")
        .with_primary_label(span, format!("this expression has type `{actual}`"))
        .with_note(format!("expected type: {expected}"))
        .with_note(format!("found type:    {actual}"))
        .with_help(type_pair_hint(expected, actual)
            .unwrap_or_else(|| "These two types are not compatible.".to_string()))
}
```

### Fix 2: `fun_return_type_mismatch`

```rust
pub fn fun_return_type_mismatch(...) -> Diagnostic {
    diagnostic_for(&TYPE_UNIFICATION_ERROR)
        ...
        .with_message("The body of this function does not match its return type.")
        .with_primary_label(span, format!("this expression has type `{actual_ret}`"))
        .with_note(format!("declared return type: {expected_ret}"))
        .with_note(format!("body type:            {actual_ret}"))
        .with_help(format!(
            "Change the return annotation to `-> {actual_ret}`, \
             or change the body to return `{expected_ret}`."
        ))
}
```

### Fix 3: `fun_param_type_mismatch`

```rust
pub fn fun_param_type_mismatch(...) -> Diagnostic {
    diagnostic_for(&TYPE_UNIFICATION_ERROR)
        ...
        .with_message(format!("Parameter {index} has the wrong type."))
        .with_primary_label(span, format!("this argument has type `{actual}`"))
        .with_note(format!("expected: {expected}"))
        .with_note(format!("found:    {actual}"))
        .with_help(type_pair_hint(expected, actual)
            .unwrap_or_else(|| format!("Expected `{expected}` as argument {index}.")))
}
```

### Fix 4: `fun_arity_mismatch`

```rust
pub fn fun_arity_mismatch(...) -> Diagnostic {
    let (direction, hint) = if actual > expected {
        ("too many", format!("Remove {} extra argument(s).", actual - expected))
    } else {
        ("too few", format!("Add {} missing argument(s).", expected - actual))
    };
    diagnostic_for(&TYPE_UNIFICATION_ERROR)
        ...
        .with_message(format!(
            "I am applying a function to {} arguments.", direction
        ))
        .with_primary_label(span, format!("this call passes {actual} argument(s)"))
        .with_note(format!("this function takes: {expected} argument(s)"))
        .with_note(format!("but this call passes: {actual} argument(s)"))
        .with_help(hint)
}
```

### Fix 5: `occurs_check_failure`

```rust
pub fn occurs_check_failure(...) -> Diagnostic {
    diagnostic_for(&OCCURS_CHECK_FAILURE)
        ...
        .with_message("I found a type that would be infinitely recursive.")
        .with_primary_label(span, "this expression causes an infinite type")
        .with_help(
            "A value cannot contain itself directly. \
             If you need self-referential data, define an ADT: \
             `type Tree = Leaf | Node(Tree, Tree)`."
        )
}
```

### Fix 6: Parser `peek_error()` elimination

Every remaining call to `peek_error()` (currently 1 site in `helpers.rs`)
is replaced with `expect_peek_context` carrying a construct-specific message
and hint. The `peek_error` function is removed entirely.

### Fix 7: `type_pair_hint` helper

A pure function in `compiler_errors.rs`:

```rust
fn type_pair_hint(expected: &str, actual: &str) -> Option<String> {
    match (expected, actual) {
        ("String", "Int") | ("String", "Float") =>
            Some(format!("Try `to_string(x)` to convert a `{actual}` to a `String`.")),
        ("Int", "Float") =>
            Some("Try `to_int(x)` to truncate a `Float` to an `Int`.".to_string()),
        ("Float", "Int") =>
            Some("Try `to_float(x)` to widen an `Int` to a `Float`.".to_string()),
        _ if expected.starts_with("Option<") && !actual.starts_with("Option<") =>
            Some(format!("Wrap this value in `Some({actual})` or return `None`.")),
        _ if actual.starts_with("Option<") && !expected.starts_with("Option<") =>
            Some("This value might be `None`. Use a `match` to unwrap it.".to_string()),
        _ => None,
    }
}
```

## Drawbacks

- Changing message text breaks all snapshot tests that include E300/E301
  diagnostics — mechanical but requires a full snapshot review pass.
- "First person" language is a style choice; not everyone prefers it.
  The principle (separated type blocks + hint) is more important than the
  pronouns.

## Rationale and alternatives

**Alternative: keep "Cannot unify" but add note lines.**
Keeping the current message and only adding note lines would be a smaller
change but leaves the core problem — the message says "Cannot unify X with Y"
which is algorithm-speak, not user-speak.

**Alternative: copy Elm's exact wording.**
Elm says "The 1st argument to `foo` is not what I expect." Flux's current
`call_arg_type_mismatch` already says "The 1st argument to `foo` has the
wrong type." — close enough. The critical missing element is the separated
type display blocks, not the exact phrasing.

## Prior art

- Elm compiler error messages — the gold standard.
- Rust's `rustc` — secondary labels + notes + suggestions, good but more
  technical than Elm.
- Gleam — similar first-person style to Elm.

## Unresolved questions

1. Should Flux use first-person "I found" or third-person "Found a mismatch"?
   Recommendation: first-person is warmer; use it for E300/E301 and match
   the existing style for E400+ which is already third-person.
2. `with_note` is confirmed supported in `src/diagnostics/builders/` — no new
   builder method needed for separated type display blocks.
3. Levenshtein fuzzy matching for identifiers: threshold needs tuning to avoid
   false suggestions (e.g. `x` matching `xs`). Recommend distance ≤ 2 and
   minimum candidate length of 3 characters.
4. Cascade suppression strategy: suppress errors whose span is fully contained
   within the span of a prior error in the same statement, or limit to N errors
   per top-level statement.

## Future possibilities

- **Type origin tracking**: Record which expression forced each type variable
  binding and display "I inferred this type because of the expression at
  line X." This requires threading `Span` through `TypeSubst` bindings.
- **`did you mean` for identifiers**: Levenshtein distance on undefined names.
- **Interactive fix suggestions**: Machine-readable `fix:` annotations that
  editors can apply automatically.
