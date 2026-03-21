- Feature Name: Perceus Uniqueness Analysis in the Compiler
- Start Date: 2026-03-01
- Status: Superseded by 0084 and 0114
- Proposal PR: pending
- Flux Issue: pending

# Proposal 0068: Perceus Uniqueness Analysis in the Compiler

## Summary
[summary]: #summary

Status note:
This proposal was not implemented as written. Its goals were absorbed into the
Core/Aether pipeline in proposal 0084, with remaining maturity work tracked in
proposal 0114.

Add a uniqueness analysis pass to the Flux compiler that determines, at each `match` site
and each function call site, whether a value is *uniquely owned* — meaning its `Rc`
reference count is guaranteed to be 1 at that point. This analysis is the prerequisite
for proposal 0069 (in-place reuse) and proposal 0070 (GcHandle elimination). It does not
change program semantics; it only annotates the AST with ownership information.

## Motivation
[motivation]: #motivation

Flux's current `Rc<T>` model means purely functional operations always allocate new
objects. Consider:

```flux
fn double_all(xs: Array<Int>) -> Array<Int> {
    map(xs, \x -> x * 2)
}
```

If `xs` is not used after this call (it is consumed), the `map` implementation could
reuse `xs`'s `Vec<Value>` in-place instead of allocating a new one. The resulting code
is as fast as imperative in-place mutation, despite being written in pure functional style.

The Perceus paper (Reinking et al., 2021) proves this is semantically safe for any value
where `Rc::strong_count == 1`. Flux's no-cycle invariant means the analysis is always
sound — there are no aliasing cycles to worry about.

This analysis, once implemented, enables:
- Proposal 0069: emit `Rc::get_mut` fast paths in the compiler backend.
- Proposal 0070: eliminate `GcHandle` by making cons/HAMT operations uniqueness-aware.
- Long-term: zero-allocation purely functional data structure operations.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### What the analysis computes

For each binding site and each use site in the AST, the analysis computes:

```
Ownership(expr) ∈ { Unique, Shared, Unknown }
```

- **Unique**: the `Rc`'s strong count is guaranteed to be 1 at this point in execution.
  Safe to call `Rc::get_mut()` without checking.
- **Shared**: the value is known to have multiple references. Cannot mutate in-place.
- **Unknown**: cannot determine statically. Must check `Rc::strong_count()` at runtime.

### When is a value Unique?

A value is **Unique** at a use site if:
1. It was just constructed at this site (new `Rc::new(...)`) — the constructor always
   produces a fresh `Rc` with count 1.
2. It was moved from a binding that is used exactly once and not captured in a closure.
3. It is a function parameter that is consumed (the caller passes ownership).
4. It was returned from a function that creates a fresh value.

A value becomes **Shared** if:
1. It is passed to more than one use site.
2. It is captured in a closure that is called more than once.
3. It is stored in a data structure that is then cloned.
4. It is returned from a function that may return the same value multiple times.

### Example: unique value at a match site

```flux
fn increment_head(xs: Array<Int>) -> Array<Int> {
    -- xs is Unique here (passed as sole argument, not aliased)
    match xs {
        [|head | rest|] -> [|head + 1 | rest|]
        _               -> xs
    }
}
```

The analysis annotates `xs` as **Unique** at the match site. Proposal 0069 will then
emit `Rc::get_mut` to modify the array in-place.

### Example: shared value

```flux
fn process(xs: Array<Int>) -> (Array<Int>, Array<Int>) {
    -- xs is Shared: used in two positions
    let a = map(xs, \x -> x + 1)
    let b = map(xs, \x -> x * 2)
    (a, b)
}
```

Here `xs` is **Shared** — it appears in two `map` calls. Both maps must allocate new
arrays; neither can reuse `xs` in-place.

### No change to user-visible behavior

This analysis is entirely internal to the compiler. No new syntax. No new errors. The
only observable effect is that some programs run faster (fewer allocations) after
proposals 0069 and 0070 use the annotations.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### New module: `src/ast/uniqueness.rs`

```rust
// src/ast/uniqueness.rs

use std::collections::HashMap;
use crate::ast::{Expression, Statement, Program};
use crate::syntax::symbol::Symbol;

/// Ownership status of a value at a specific AST node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ownership {
    /// Rc::strong_count is guaranteed to be 1.
    Unique,
    /// Rc::strong_count is >= 2 (multiple references).
    Shared,
    /// Cannot determine statically; must check at runtime.
    Unknown,
}

/// Per-expression ownership annotation.
/// Keyed by the raw pointer of the Expression (same key as hm_expr_types uses).
pub type OwnershipMap = HashMap<*const Expression, Ownership>;

/// Run uniqueness analysis over a compiled program.
/// Returns an OwnershipMap that the code generator can query.
pub fn analyze_uniqueness(program: &Program) -> OwnershipMap {
    let mut ctx = UniqCtx::new();
    for stmt in &program.statements {
        ctx.analyze_statement(stmt);
    }
    ctx.ownership
}

struct UniqCtx {
    /// Maps binding name → current ownership of the bound value.
    env: Vec<HashMap<Symbol, Ownership>>,  // scope stack
    /// Result: ownership of each expression.
    ownership: OwnershipMap,
    /// Use-count for each binding in the current scope.
    use_counts: Vec<HashMap<Symbol, usize>>,
}

impl UniqCtx {
    fn new() -> Self {
        Self {
            env: vec![HashMap::new()],
            ownership: HashMap::new(),
            use_counts: vec![HashMap::new()],
        }
    }

    fn analyze_expression(&mut self, expr: &Expression) -> Ownership {
        let own = match expr {
            // Constructors always produce fresh Rc (count = 1 = Unique)
            Expression::Array { .. }
            | Expression::Tuple { .. }
            | Expression::StringLiteral { .. } => Ownership::Unique,

            // Literals have no Rc at all; treat as Unique (trivially copyable)
            Expression::Integer { .. }
            | Expression::Float { .. }
            | Expression::Boolean { .. }
            | Expression::None { .. } => Ownership::Unique,

            // Identifier: look up use-count in scope
            Expression::Identifier { name, .. } => {
                let count = self.increment_use_count(*name);
                if count == 1 {
                    // First use: might still be Unique if binding is Unique
                    self.env_lookup(*name).unwrap_or(Ownership::Unknown)
                } else {
                    // Second or later use: definitely Shared (value is referenced elsewhere)
                    Ownership::Shared
                }
            }

            // Function calls: result is Unique if the function always creates fresh values
            Expression::Call { callee, arguments, .. } => {
                // Analyze arguments to update their use counts
                for arg in arguments {
                    self.analyze_expression(arg);
                }
                // Result of a call is Unknown unless we can prove it's a constructor call
                // (conservatively Unknown for now; proposal 0069 refines this)
                Ownership::Unknown
            }

            // Lambda: the closure itself is Unique when created
            Expression::Lambda { body, .. } => {
                // Analyze body to track captures, but result is a fresh closure
                self.analyze_expression(body);
                Ownership::Unique
            }

            // Match: ownership of the matched value propagates to each arm's binding
            Expression::Match { subject, arms, .. } => {
                let subject_own = self.analyze_expression(subject);
                self.ownership.insert(subject as *const Expression, subject_own);

                let mut result_own = Ownership::Unknown;
                for arm in arms {
                    // In each arm, the destructured bindings have ownership derived
                    // from the subject's ownership.
                    self.push_scope();
                    self.bind_pattern_ownership(&arm.pattern, subject_own);
                    let arm_own = self.analyze_expression(&arm.body);
                    result_own = ownership_join(result_own, arm_own);
                    self.pop_scope();
                }
                result_own
            }

            // Let: analyze the bound expression, record ownership for the name
            Expression::Let { name, value, body, .. } => {
                let val_own = self.analyze_expression(value);
                self.push_scope();
                self.env.last_mut().unwrap().insert(*name, val_own);
                self.use_counts.last_mut().unwrap().insert(*name, 0);
                let result = self.analyze_expression(body);
                // After analyzing body, if use_count == 1, binding was unique-used
                let used_once = self.use_counts.last().unwrap()
                    .get(name).copied().unwrap_or(0) <= 1;
                if !used_once {
                    // Used more than once: retroactively mark as Shared
                    self.ownership.insert(value as *const Expression, Ownership::Shared);
                }
                self.pop_scope();
                result
            }

            // If/else: ownership is the join of both branches
            Expression::If { condition, then_branch, else_branch, .. } => {
                self.analyze_expression(condition);
                let t = self.analyze_expression(then_branch);
                let e = else_branch.as_ref()
                    .map(|b| self.analyze_expression(b))
                    .unwrap_or(Ownership::Unique);
                ownership_join(t, e)
            }

            _ => Ownership::Unknown,
        };

        self.ownership.insert(expr as *const Expression, own);
        own
    }

    fn analyze_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Function { body, parameters, .. } => {
                self.push_scope();
                // Parameters: Unique if the function is the sole caller-path;
                // conservatively Unknown for public functions.
                for param in parameters {
                    self.env.last_mut().unwrap().insert(param.name, Ownership::Unknown);
                }
                self.analyze_expression(body);
                self.pop_scope();
            }
            Statement::Let { name, value, .. } => {
                let own = self.analyze_expression(value);
                self.env.last_mut().unwrap().insert(*name, own);
            }
            _ => {}
        }
    }

    fn bind_pattern_ownership(&mut self, pattern: &Pattern, subject_own: Ownership) {
        // When destructuring, fields inherit the subject's ownership.
        // E.g., if xs is Unique and we match [h | t], both h and t are Unique.
        match pattern {
            Pattern::Identifier(name) => {
                self.env.last_mut().unwrap().insert(*name, subject_own);
                self.use_counts.last_mut().unwrap().insert(*name, 0);
            }
            Pattern::Tuple(fields) | Pattern::Array(fields) => {
                for f in fields {
                    self.bind_pattern_ownership(f, subject_own);
                }
            }
            Pattern::Cons(head, tail) => {
                self.bind_pattern_ownership(head, subject_own);
                self.bind_pattern_ownership(tail, subject_own);
            }
            _ => {}
        }
    }

    fn increment_use_count(&mut self, name: Symbol) -> usize {
        for scope in self.use_counts.iter_mut().rev() {
            if let Some(count) = scope.get_mut(&name) {
                *count += 1;
                return *count;
            }
        }
        1  // Not found in scope: external binding, count as 1
    }

    fn env_lookup(&self, name: Symbol) -> Option<Ownership> {
        for scope in self.env.iter().rev() {
            if let Some(own) = scope.get(&name) {
                return Some(*own);
            }
        }
        None
    }

    fn push_scope(&mut self) {
        self.env.push(HashMap::new());
        self.use_counts.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.env.pop();
        self.use_counts.pop();
    }
}

/// Lattice join: if either side is Shared, result is Shared.
fn ownership_join(a: Ownership, b: Ownership) -> Ownership {
    match (a, b) {
        (Ownership::Shared, _) | (_, Ownership::Shared) => Ownership::Shared,
        (Ownership::Unique, Ownership::Unique) => Ownership::Unique,
        _ => Ownership::Unknown,
    }
}
```

### Integration in the compiler pipeline

The uniqueness analysis runs after HM inference (which already annotates expression types)
and before PASS 2 (code generation):

```rust
// src/bytecode/compiler/mod.rs — in compile_program():

// After HM inference:
let ownership_map = if self.config.perceus_enabled {
    analyze_uniqueness(program)
} else {
    HashMap::new()
};
self.ownership_map = ownership_map;

// PASS 2 can now query:
//   self.ownership_map.get(&(expr as *const Expression))
//   to decide whether to emit Rc::get_mut or clone
```

### Feature flag

Controlled by `--perceus` CLI flag (off by default until proposal 0069 is complete):

```toml
# Cargo.toml — no feature flag needed; controlled at runtime via CLI
```

```rust
// src/main.rs
.arg(clap::Arg::new("perceus")
    .long("perceus")
    .help("Enable Perceus uniqueness analysis and in-place reuse (experimental)")
    .action(ArgAction::SetTrue))
```

### Validation

```bash
# Build with uniqueness analysis
cargo build

# Run analysis on a test file and dump ownership annotations (new subcommand)
cargo run -- analyze-ownership examples/type_system/my_program.flx

# Run all uniqueness analysis tests
cargo test --test uniqueness_tests
```

## Drawbacks
[drawbacks]: #drawbacks

- The analysis is conservative. Many values that are actually unique at runtime will be
  annotated as `Unknown` and miss the optimization.
- Use-count tracking via raw `*const Expression` pointers is fragile across AST
  transformations. The analysis must run after all AST transforms are complete.
- The analysis does not track uniqueness through function boundaries (inter-procedural
  uniqueness requires whole-program analysis — deferred to a future proposal).

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Why not runtime `Rc::strong_count` checks everywhere?** Runtime checks add a branch
per operation. Static analysis eliminates the check entirely for `Unique` values and
moves the check to `Unknown` values only. Runtime-only checking is proposal 0069's
fallback for the `Unknown` case.

**Why intra-procedural only?** Inter-procedural uniqueness (tracking ownership across
function call boundaries) requires either whole-program analysis or a separate ownership
type annotation in function signatures. The Perceus paper also starts with intra-
procedural. This is the correct phased approach.

**Relationship to Rust's borrow checker:** Rust enforces uniqueness at compile time with
the borrow checker. Flux cannot use Rust's borrow checker for Flux values (they are
`Rc<T>`, not `&T`). This analysis reimplements the relevant portion of uniqueness
checking at the Flux AST level.

## Prior art
[prior-art]: #prior-art

- **Perceus** (Reinking, Xie, Leijen, Swamy, 2021) — the formal basis for this analysis.
  "Perceus: Garbage Free Reference Counting with Reuse". PLDI 2021.
- **Koka compiler** — implements Perceus as a compiler pass over the core IR.
- **Clean language** — uniqueness types as a first-class language feature. Flux's
  approach is to infer them without requiring user annotations.
- **Rust borrow checker** — the most prominent uniqueness enforcement system.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Should the `analyze-ownership` subcommand output be human-readable or machine-readable
   (JSON)? Decision: human-readable for development; JSON flag for tooling.
2. Should uniqueness be tracked through closures? If a lambda captures `xs` and the
   lambda is called once, `xs` may still be unique inside the lambda. This requires
   capturing the closure's call-count analysis. Deferred.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Inter-procedural uniqueness**: annotate function signatures with ownership contracts
  (`fn f(xs: unique Array<Int>) -> Array<Int>`) and propagate across call boundaries.
- **Uniqueness types in the surface language**: expose uniqueness as a user-visible type
  annotation for performance-critical code, similar to Clean language.
- **Proposal 0069**: uses this analysis to emit in-place reuse in the code generator.
- **Proposal 0070**: uses this analysis to make GC heap operations uniqueness-aware.
