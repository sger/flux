- Feature Name: 004_language_features_proposal
- Start Date: 2026-02-26
- Status: Partially Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0004: 004_language_features_proposal

## Summary
[summary]: #summary

This proposal defines the scope and delivery model for 004_language_features_proposal in Flux. It consolidates the legacy specification into the canonical proposal template while preserving technical and diagnostic intent.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Design Philosophy

Flux aims to be a **functional-first** language with these guiding principles:

1. **Data flows through transformations** - The name "Flux" reflects this
2. **Expressions over statements** - Everything returns a value
3. **Immutability by default** - Explicit when mutation occurs
4. **Effects are tracked** - Side effects are visible in the type system
5. **Concurrency is safe** - Actor model for isolation

### Syntax Style

- **Clean and minimal** - Avoid unnecessary syntax noise
- **Consistent** - Similar constructs look similar
- **Readable** - Code reads like documentation
- **Familiar** - Draw from established FP languages

### 1.10 Range Syntax (Medium Priority)

**Proposed syntax:**
```flux
// Exclusive range (end not included)
1..10        // 1, 2, 3, 4, 5, 6, 7, 8, 9

// Inclusive range
1..=10       // 1, 2, 3, 4, 5, 6, 7, 8, 9, 10

// With step
0..10..2     // 0, 2, 4, 6, 8
10..0..-1    // 10, 9, 8, ..., 1

// Open ranges (when context provides bounds)
..10         // 0 to 9
5..          // 5 to end

// Usage
for i in 1..=5 {
    print(i)
}

let slice = arr[2..5]    // elements 2, 3, 4
let chars = str[0..3]    // first 3 characters
```

**Implementation:**
- `Range` object type: `{ start, end, step, inclusive }`
- Lazy evaluation (doesn't create full list)
- Works with `for`, array slicing, list comprehensions

### Design Goals

- Pure functions by default
- Effects are visible in signatures
- Effects can be handled/mocked
- Composable effect tracking

### Stream Syntax

```flux
// Stream literal
stream {
    yield 1;
    yield 2;
    yield 3;
}

// Async stream
stream {
    for url in urls {
        let data = await fetch(url);
        yield data;
    }
}

// Reactive bindings (re-evaluate when dependencies change)
let~ count = 0;
let~ doubled = count * 2;  // automatically updates

// Subscribe
subscribe(stream, \value -> {
    print("Received: #{value}");
});

// Or with |>
stream |> subscribe(\value -> handle(value));
```

### Phase 1: Essential Syntax (Weeks 1-4)

| Feature | Effort | Priority |
|---------|--------|----------|
| Operators: `<=`, `>=`, `&&`, `\|\|`, `%` | Small | Critical |
| Pipe operator `\|>` | Small | Critical |
| Block comments `/* */` | Small | Medium |
| Lambda shorthand `\x -> expr` | Medium | High |

### Open Design Questions

1. **Lambda syntax**: `\x -> expr` vs `|x| expr` vs `fn(x) expr`?

2. **Effect syntax**: `with Effect` vs `!Effect` vs `@Effect`?

3. **Actor syntax**: Keyword `actor` vs `process` vs `agent`?

4. **Stream syntax**: `stream { yield x }` vs generator functions?

5. **Type parameter syntax**: `<T>` vs `[T]` vs `{T}`?

6. **Visibility modifiers**: `pub`/`priv` vs `_prefix` convention vs explicit `export`?

7. **Mutability**: `let mut` vs `var` vs `let!`?

### Design Philosophy

### Syntax Style

### 1.10 Range Syntax (Medium Priority)

### Design Goals

### Stream Syntax

### Phase 1: Essential Syntax (Weeks 1-4)

### Open Design Questions

5. **Type parameter syntax**: `<T>` vs `[T]` vs `{T}`?

7. **Mutability**: `let mut` vs `var` vs `let!`?

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **__preamble__:** This document outlines proposed language features and syntax improvements for Flux, organized by priority and implementation complexity. - **Table of Content...
- **Flux Language Features Proposal:** This document outlines proposed language features and syntax improvements for Flux, organized by priority and implementation complexity.
- **Table of Contents:** 1. [Design Philosophy](#design-philosophy) 2. [Part I: Syntax Improvements](#part-i-syntax-improvements) 3. [Part II: Core Language Features](#part-ii-core-language-features) 4....
- **Comparison Operators: `<=` and `>=`:** **Current limitation:** ```flux // Cannot write this if n <= 0 { ... }
- **Logical Operators: `&&` and `||`:** **Current limitation:** ```flux // Cannot write this if a > 0 && b > 0 { ... }
- **Modulo Operator: `%`:** **Current limitation:** ```flux // Cannot check if even/odd // Cannot wrap around values ```

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

1. Restructuring legacy material into a strict template can reduce local narrative flow.
2. Consolidation may temporarily increase document length due to historical preservation.
3. Additional review effort is required to keep synthesized sections aligned with implementation changes.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

No additional prior art identified beyond references already listed in the legacy content.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

### With Type Parameters (Future)

```flux
type Tree<T> {
    Leaf(T)
    Node(Tree<T>, T, Tree<T>)
}

fn map_tree<T, U>(tree: Tree<T>, f: T -> U): Tree<U> {
    match tree {
        Leaf(x) -> Leaf(f(x));
        Node(left, x, right) ->
            Node(map_tree(left, f), f(x), map_tree(right, f));
    }
}
```

### Type Annotation (Future)

```flux
let point: (Int, Int) = (10, 20);

fn swap<A, B>(pair: (A, B)): (B, A) {
    let (a, b) = pair;
    (b, a)
}
```

### With Type Parameters (Future)

### Type Annotation (Future)
