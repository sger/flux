- Feature Name: Typed Holes
- Start Date: 2026-03-08
- Status: Draft
- Proposal PR:
- Flux Issue:
- Depends on: 0032, 0080, 0081

# Proposal 0083: Typed Holes

## Summary

Add typed holes to Flux with syntax like `?name` and `?`, allowing programmers to leave intentional expression placeholders in code and ask the compiler what type of expression belongs there. A typed hole is always a compile-time error, but it is a helpful one: Flux should report the expected type, relevant in-scope candidates, and a small amount of guided next-step information.

This is a pure developer-experience feature. It has no runtime semantics and does not weaken purity, referential transparency, or type safety.

## Motivation

In pure functional programming, programmers often know the shape of a program before they know the exact expression that fills every position.

For example:

```flux
fn load_orders(path: String) -> List<Order> {
    read_lines(path)
        |> map(?parse_line)
        |> filter(?is_valid)
}
```

This is not "unfinished code by accident." It is a normal way to design functional programs:

- sketch the pipeline first
- let the types drive the missing pieces
- fill in the holes once the compiler explains what they need to be

Without typed holes, the compiler only reports that the program is incomplete or wrong. With typed holes, the compiler becomes an active guide:

- what type is required here?
- what bindings in scope already fit?
- what lambda skeleton would match this position?

This fits Flux especially well because Flux already values:

- HM-style type inference
- expression-oriented programming
- functional composition
- high-quality diagnostics

Typed holes would let Flux turn those strengths into a memorable workflow feature.

## Guide-level explanation

### Basic syntax

Flux would allow two kinds of hole expressions:

```flux
?name
?
```

Examples:

```flux
let formatter = ?fmt
```

```flux
items |> map(?)
```

```flux
handle Network {
    timeout(resume, req) -> ?fallback
}
```

Named holes are preferred because they make diagnostics easier to read.

### What the compiler should do

A hole is always a compile-time error, but Flux should treat it as an intentional request for help.

For:

```flux
fn total(xs: List<Int>) -> Int {
    foldl(?step, 0, xs)
}
```

Flux should report something like:

```text
Error[H001]: Typed Hole

I found a hole named `?step`.

This hole needs an expression of type:

  Int -> Int -> Int

These values are in scope and may fit:

- add : Int -> Int -> Int
- max : Int -> Int -> Int

You could also start with:

  \acc, x -> ...
```

The point is not merely to reject the program. The point is to help the programmer continue writing it.

### Why this belongs in pure FP

Typed holes fit pure functional programming extremely well:

- they are type-directed
- they support top-down expression design
- they encourage composition-first development
- they have no runtime meaning

They do not introduce impurity. They are simply a compile-time affordance for writing typed programs more effectively.

### Recommended syntax

The recommended syntax is:

- `?name` for named holes
- `?` for anonymous holes

This is recommended because it is:

- short
- visually distinctive
- easy to read in expression-heavy code
- conventional enough for users familiar with typed-FP tooling

### Alternative syntax

Alternative spellings could include:

- `_?name`
- `_hole`
- `todo<name>`

These are all weaker than `?name`.

`_?name` is more explicit but uglier.
`_hole` is more keyword-like but less elegant in expression pipelines.
`todo<name>` looks more like a macro or placeholder API than syntax.

## Reference-level explanation

### Surface syntax

Add a new expression form:

```text
hole_expr ::= "?"
            | "?" identifier
```

This should parse anywhere a normal expression can appear.

Examples of valid positions:

- call arguments
- let initializers
- return expressions
- pipeline stages
- match arms
- handler bodies
- data structure literals

Typed holes are expression placeholders, not statement placeholders.

### AST representation

Add a dedicated AST node, for example:

```rust
Expression::Hole {
    name: Option<Symbol>,
    span: Span,
}
```

This should remain explicit through typing so the compiler can attach precise diagnostics and gather local context.

### Typing behavior

When the type checker encounters a hole:

1. infer or constrain the expected type at that location
2. record a typed-hole diagnostic instead of crashing or emitting a generic mismatch
3. continue compilation where feasible so multiple holes can be reported in one pass

The hole itself should not be treated as `Any` and should not silently satisfy constraints. It must remain a hard compile error.

The compiler should report:

- hole name, if present
- expected type at the hole site
- source span
- selected candidate bindings from local scope
- optionally a lambda skeleton when the expected type is a function type

### Candidate suggestions

Candidate reporting should stay conservative.

Recommended policy:

- include nearby/local bindings first
- prefer exact type matches
- then near matches with small explanatory notes
- cap the candidate count to keep diagnostics readable
- do not flood the user with every in-scope binding

Possible ranking dimensions:

- exact type equality
- successful unification without introducing unconstrained `Any`
- lexical proximity / local scope priority
- human-readable name quality

For function-typed holes, Flux may additionally suggest a starter lambda skeleton:

```text
\arg1, arg2 -> ...
```

This should be derived from the arity of the expected type.

### Diagnostics

Typed holes should get a dedicated error code family rather than reusing generic parser or type mismatch codes. For example:

- `H001` or `E430` for a general typed-hole diagnostic

The exact code is open, but it should be stable and distinct.

The title should be explicit:

- `Typed Hole`

The diagnostic category should be:

- `TypeInference`

The message should be human-first and structured:

- identify the hole
- show the expected type
- show candidate fits
- show an optional lambda skeleton

### Multi-hole behavior

If a file contains multiple independent holes, Flux should report all of them when possible.

Example:

```flux
fn build(x: Int) -> String {
    let f = ?formatter
    ?result
}
```

This should ideally report two typed holes, not only the first one.

### Interaction with existing diagnostics

Typed holes should take precedence over derivative type mismatch noise at the same span.

If a hole exists, Flux should prefer the typed-hole diagnostic rather than:

- generic type mismatch fallout
- unresolved variable noise
- downstream effect errors caused only by the missing expression

But unrelated independent errors elsewhere in the file should still be reported.

### Implementation path

The intended implementation order is:

1. parser support for `?` and `?name`
2. AST representation for holes
3. type-checker handling that preserves expected type information
4. typed-hole diagnostic rendering
5. candidate search from scope
6. multi-hole reporting and suppression tuning

## Drawbacks

Typed holes increase language surface area for a feature that exists only during development.

They also raise expectations for diagnostic quality. A weak typed-hole implementation would feel gimmicky rather than powerful.

Candidate ranking is another source of complexity. If suggestions are poor or noisy, the feature will feel less helpful than intended.

## Rationale and alternatives

The main alternative is to do nothing and let users rely on ordinary type errors while writing incomplete code.

That is simpler, but much less helpful. It keeps the compiler in a passive role.

Another alternative is to provide a library-level placeholder like `todo()` or `hole()`. That is weaker because:

- it cannot parse everywhere naturally as an expression placeholder
- it looks like runtime code instead of a compile-time tool
- it cannot carry the same lightweight syntax or tailored diagnostics

The proposed design is better because it treats typed holes as a first-class language feature with first-class diagnostics.

## Prior art

The clearest prior art is Haskell's typed holes in GHC.

Typed holes are also familiar in dependently typed languages and theorem-prover environments such as Idris and Agda, where placeholders are part of normal top-down development.

Flux should not copy those systems verbatim. The opportunity is to bring the same underlying idea into a cleaner, more diagnostics-first user experience:

- more human-facing reporting
- better integration with pipeline-heavy functional code
- clearer suggestions for users who are not already experts in advanced type-system tooling

## Unresolved questions

- Should typed holes use a new diagnostic code prefix such as `H001`, or stay within the `E` code space?
- How aggressively should Flux suggest in-scope candidates before the output becomes noisy?
- Should anonymous `?` be supported in v1, or only named holes like `?name`?
- Should typed holes emit lambda skeleton suggestions in v1, or should that wait until candidate ranking is stable?

## Future possibilities

If typed holes land well, Flux could later extend the feature with:

- editor actions that insert a suggested candidate automatically
- richer "best fit" ranking using name similarity and module proximity
- goal-directed synthesis for very small expressions
- hole-driven workflow in REPL or IDE tooling

The v1 goal is smaller: make incomplete typed Flux programs meaningfully explorable through the compiler.
