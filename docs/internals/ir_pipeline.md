# Flux IR Pipeline: Flux Core and Backend IR

This document explains the two long-term IR layers that sit between the surface
AST and executable code in Flux, and how they interact.

---

## Overview

```
                        ┌─────────────────────────────────────────┐
  Surface AST           │            Intermediate Representations │
  (many variants,       │                                         │
   syntactic sugar)     │  Flux Core         Backend IR           │     Executable
                        │  ~12 variants      basic blocks         │
        ──────────►     │  functional        imperative           │  ──────────►
                        │  tree-shaped       graph-shaped         │
                        │                                         │     Bytecode
                        │  Optimization:     Code generation:     │        or
                        │  beta reduce,      opcodes, jumps,      │     Native
                        │  COKC, inline      register alloc       │
                        └─────────────────────────────────────────┘
```

---

## Flux Core

> Canonical API: `src/core/`

Flux Core is the semantic IR of the compiler. All of Flux's surface constructs
— function definitions, if/else, match expressions, operators, lambdas, list
comprehensions — are desugared into **~12 expression variants**:

```
Surface Flux                 →  Core IR
─────────────────────────────────────────────────────────────
fn f(x, y) { ... }          →  let f = Lam([x, y], ...)
f(a, b)                     →  App(f, [a, b])
if cond then a else b        →  Case(cond, [True→a, False→b])
match p { A(x) → e }        →  Case(p, [Con(A,[x])→e])
let x = e; body              →  Let(x, e, body)
x + y                       →  PrimOp(Add, [x, y])
perform Eff.op(a)            →  Perform(Eff, op, [a])
handle { body } with Eff { } →  Handle(body, Eff, handlers)
```

### Key types

| Type | Purpose |
|------|---------|
| `CoreExpr` | The central expression type — 11 variants covering all computation |
| `CoreLit` | Literals: Int, Float, Bool, String, Unit |
| `CoreTag` | Constructor tags: Named, None, Some, Left, Right, Nil, Cons |
| `CorePrimOp` | ~30 primitive operations including typed variants (IAdd, FMul, etc.) |
| `CorePat` | Patterns: Wildcard, Lit, Var, Con, Tuple, EmptyList |
| `CoreAlt` | Case alternative: pattern + optional guard + body |
| `CoreHandler` | Effect handler arm: operation + params + resume + body |
| `CoreDef` | Top-level definition: name + expression + is_recursive flag |
| `CoreProgram` | A program: sequence of `CoreDef`s |

### CoreExpr variants

```rust
enum CoreExpr {
    Var(Identifier, Span),           // Variable reference
    Lit(CoreLit, Span),              // Literal constant
    Lam { params, body, span },      // N-ary lambda
    App { func, args, span },        // N-ary application
    Let { var, rhs, body, span },    // Non-recursive binding
    LetRec { var, rhs, body, span }, // Recursive binding (self-referential functions)
    Case { scrutinee, alts, span },  // Pattern matching — the ONLY branching construct
    Con { tag, fields, span },       // Constructor application
    PrimOp { op, args, span },       // Primitive operation (replaces all operators)
    Perform { effect, op, args },    // Algebraic effect operation
    Handle { body, effect, handlers }, // Effect handler installation
}
```

### Typed primitive operations

The Core IR carries type information from HM inference into its primitive operations.
When the inferred type is concretely `Int` or `Float`, the lowering emits typed variants
instead of generic ones:

| Generic | Int variant | Float variant |
|---------|------------|---------------|
| `Add` | `IAdd` | `FAdd` |
| `Sub` | `ISub` | `FSub` |
| `Mul` | `IMul` | `FMul` |
| `Div` | `IDiv` | `FDiv` |
| `Mod` | `IMod` | — |

This allows backends to emit specialized instructions without runtime type dispatch.

### Lowering: AST → Core IR

`core/lower_ast.rs` — `lower_program_ast(program, hm_expr_types) → CoreProgram`

This runs immediately after HM type inference. Key decisions:

- **Type-directed**: The `hm_expr_types` map (from type inference) guides lowering.
  When the inferred type of an arithmetic expression is `Int`, the lowering emits `IAdd`
  instead of generic `Add`.
- **Core-owned declarations**: Top-level `Module`, `Data`, and `EffectDecl`
  statements are preserved in Core-owned top-level metadata so backend IR and
  production backends do not need AST-only declaration knowledge.
- **Nested local functions**: Block-local `fn` statements lower into Core
  `LetRec` rather than relying on an AST fallback path.
- **Desugaring**: All syntactic sugar is eliminated. `if/else` becomes `Case` with
  boolean alternatives. `match` becomes `Case` with constructor patterns. Operators
  become `PrimOp`.

### Optimization passes

`core/passes.rs` — `run_core_passes(program)`

Four passes run in order:

1. **Beta reduction** — Eliminates `App(Lam([x], body), [arg])` by substituting `arg` for `x`
   in `body`. Only fires when the lambda has a single use.

2. **Case-of-known-constructor (COKC)** — When `Case` scrutinizes a literal or constructor
   whose value is known at compile time, selects the matching alternative statically and
   eliminates the branch.

3. **Inline trivial lets** — Substitutes `Let(x, Lit(n), body)` and `Let(x, Var(y), body)`
   directly. COKC often creates these as intermediate bindings.

4. **Dead let elimination** — Drops `Let` bindings where the variable is never referenced
   in the body. Only safe for pure bindings.

### Core IR → CFG IR lowering

`core/to_ir.rs` — `lower_core_to_ir(core) → IrProgram`

Translates functional `CoreExpr` trees into imperative CFG basic blocks:

- **Uncurrying**: Top-level `Lam` chains become multi-parameter `IrFunction`s
- **Closure conversion**: `Lam` inside expressions becomes `MakeClosure(fn_id, captures)`.
  The inner function receives captures as its first parameters, accessed via `OpGetFree`.
- **Case compilation**: Patterns become sequences of tag/literal tests and conditional jumps
- **Let flattening**: Nested `Let` expressions become sequential `IrInstr::Assign` in blocks

---

## Backend IR

> Canonical API: `src/backend_ir/`
>
> Current implementation: `src/cfg/`
>
> Canonical IR IDs live in `src/shared_ir/`.
>
> `cfg/` is the private backend engine; production backend traffic flows
> through `backend_ir/`.

The current backend IR is the CFG (Control Flow Graph) representation consumed
by both the bytecode compiler and the Cranelift JIT. Programs are represented
as collections of **functions**, each containing **basic blocks** connected by
jumps and branches.

`src/ir/` has been retired from this pipeline.

### What is a Control Flow Graph?

A CFG represents code as a directed graph where:

- Each **node** is a **basic block** — a straight-line sequence of instructions with no
  branching in the middle
- Each **edge** is a **control flow transfer** — a jump, conditional branch, return, or
  tail call
- Every basic block ends with exactly one **terminator** instruction

This is the standard representation used by most production compilers (LLVM, Cranelift, GCC)
because it makes optimizations and code generation systematic.

### Key types

| Type | Purpose |
|------|---------|
| `IrProgram` | Complete production backend program: functions + top-level items + globals + HM types |
| `IrFunction` | A function: params, captures, basic blocks, entry block, metadata |
| `IrBlock` | A basic block: params + instructions + terminator |
| `IrInstr` | An instruction: Assign, Call, or HandleScope |
| `IrTerminator` | Block ending: Jump, Branch, Return, TailCall, Unreachable |
| `IrExpr` | Right-hand side of an assignment (constants, binary ops, constructors, etc.) |
| `IrVar` | SSA-like variable reference (u32 ID) |
| `BlockId` | Basic block identifier (u32 ID) |
| `FunctionId` | Function identifier (u32 ID) |
| `IrTopLevelItem` | Top-level metadata: Let, Function, Module, Import, Data, EffectDecl |

### IrInstr — instructions

```rust
enum IrInstr {
    // Variable assignment: dest = expr
    Assign { dest: IrVar, expr: IrExpr, metadata: IrMetadata },

    // Function call: dest = target(args...)
    Call { dest: IrVar, target: IrCallTarget, args: Vec<IrVar>, metadata: IrMetadata },

    // Effect handler scope (installs handler, executes body blocks, removes handler)
    HandleScope { effect, arms, body_entry, body_result, dest, metadata },
}
```

### IrTerminator — block endings

```rust
enum IrTerminator {
    Jump(BlockId, Vec<IrVar>, IrMetadata),           // Unconditional jump with arguments
    Branch { cond, then_block, else_block, metadata }, // Conditional branch
    Return(IrVar, IrMetadata),                         // Return value from function
    TailCall { callee, args, metadata },               // Tail call optimization
    Unreachable(IrMetadata),                           // Dead code marker
}
```

### IrExpr — expressions (assignment right-hand sides)

The `IrExpr` enum covers all value-producing operations:

- **Constants**: `Const(IrConst)`, `None`, `EmptyList`
- **Variables**: `Var(IrVar)`, `LoadName(Identifier)`
- **Arithmetic**: `Binary(IrBinaryOp, lhs, rhs)`, `Prefix(op, operand)`
- **Collections**: `MakeTuple`, `MakeArray`, `MakeHash`, `MakeList`, `MakeAdt`
- **Access**: `Index`, `MemberAccess`, `TupleFieldAccess`, `AdtField`
- **Pattern tests**: `TagTest`, `ListTest`, `TupleArityTest`, `AdtTagTest`
- **Destructuring**: `TagPayload`, `ListHead`, `ListTail`
- **Wrappers**: `Some`, `Left`, `Right`, `Cons`
- **Closures**: `MakeClosure(FunctionId, captures)`
- **Effects**: `Perform`, `Handle`
- **Strings**: `InterpolatedString`

### Call targets

```rust
enum IrCallTarget {
    Direct(FunctionId),  // Known function — enables direct call optimization
    Named(Identifier),   // Named function — resolved at bytecode compilation
    Var(IrVar),          // Dynamic call — function value in a variable
}
```

### IrFunction structure

Each function contains:

```rust
struct IrFunction {
    id: FunctionId,
    name: Option<Identifier>,
    params: Vec<IrParam>,           // Named parameters with types
    parameter_types: Vec<Option<TypeExpr>>,  // Source type annotations
    return_type_annotation: Option<TypeExpr>,
    effects: Vec<EffectExpr>,       // Effect annotations
    captures: Vec<Identifier>,      // Free variables (for closures)
    blocks: Vec<IrBlock>,           // Basic blocks
    entry: BlockId,                 // Entry block
    origin: IrFunctionOrigin,       // ModuleTopLevel, NamedFunction, or FunctionLiteral
}
```

### Example: Fibonacci in CFG IR

```flux
fn fib(n) {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}
```

Compiles to approximately:

```
fn fib [NamedFunction]
  b0(v0: Int):                     // entry block, v0 = n
    v1 = Const(2)
    v2 = Binary(Lt, v0, v1)        // n < 2
    Branch v2 ? b1 : b2            // if/else

  b1():                             // then branch
    Return v0                       // return n

  b2():                             // else branch
    v3 = Const(1)
    v4 = Binary(ISub, v0, v3)      // n - 1
    v5 = call fib(v4)              // fib(n - 1)
    v6 = Const(2)
    v7 = Binary(ISub, v0, v6)      // n - 2
    v8 = call fib(v7)              // fib(n - 2)
    v9 = Binary(IAdd, v5, v8)      // fib(n-1) + fib(n-2)
    Return v9
```

### Closures in CFG IR

When a lambda captures variables from an enclosing scope, the Core IR → CFG IR lowering
performs **closure conversion**:

```flux
fn make_adder(x) {
    /y -> x + y
}
```

Produces two `IrFunction`s:

1. `make_adder`: Contains `MakeClosure(fn_id, [v_x])` — packages `x` as a capture
2. Anonymous inner function: Parameters are `[x_capture, y]` — captures come first

At the bytecode level:
- Capture parameters use `SymbolScope::Free` → emit `OpGetFree` instructions
- Real parameters use `SymbolScope::Local` → emit `OpGetLocal` instructions
- The VM stores free variables in the closure object, not on the stack

### Debugging CFG IR

Use the `ir` subcommand to inspect the CFG IR for any source file:

```bash
cargo run -- ir examples/basics/fibonacci.flx
```

This prints the text representation of all `IrFunction`s with block structure,
instructions, and terminators. The output uses `IrProgram::dump_text_with_interner()`
for human-readable names.

---

## Structured IR

`structured_ir` has been retired. Production lowering and execution now flow
through:

- `core/lower_ast.rs`
- `core/to_ir.rs`
- `backend_ir/`
- bytecode/JIT codegen

---

## Backend Program Assembly

The production backend program is assembled from Core-backed lowering:

1. `core/lower_ast.rs` produces `CoreProgram`
2. `core/passes.rs` simplifies/normalizes Core
3. `core/to_ir.rs` lowers Core into backend `IrProgram`
4. `backend_ir` pass/validation/codegen layers consume that `IrProgram`

Top-level declaration metadata such as modules, data declarations, and effect
declarations is now carried through Core and emitted into backend
`IrTopLevelItem`s, so production backends do not need to reconstruct those
shapes from the source AST.

---

## Pipeline Inspection

### Inspect Core IR

Currently, Core IR can be inspected via the `display` module. The `CoreProgram` and
`CoreExpr` types implement pretty-printing in `core/display.rs`.

### Inspect CFG IR

```bash
cargo run -- ir examples/basics/fibonacci.flx
```

### Inspect bytecode

```bash
cargo run -- bytecode examples/basics/fibonacci.flx
```

### Full pipeline trace

```bash
cargo run -- examples/basics/fibonacci.flx --trace
```

Prints every VM instruction as it executes.

---

## Known Limitations

### Core IR coverage gaps

The remaining gaps are no longer “module functions require AST fallback” gaps.
Current limitations are narrower backend/JIT lowering coverage issues, for
example some handler/capture-heavy backend forms.

### "missing CFG" warnings

During bytecode compilation, if a CFG IR binding, function, or block cannot be found,
the compiler emits a diagnostic warning (e.g. `"missing CFG bytecode closure function"`).
These are **non-fatal** — the compiler falls back to the AST compilation path.

Common causes:
- A closure function was not included in the merged function list
- The Core IR lowering produced a different function ID than expected
- Module-internal closures are referenced but their `IrFunction` was not preserved

### JIT-specific gaps

Production JIT no longer reconstructs top-level source AST and no longer has an
AST fallback path. JIT compilation now succeeds or fails from backend IR only.

Remaining JIT issues therefore indicate real backend-path coverage bugs, not a
fallback handoff. The most likely areas are:

- handler/capture-heavy backend forms
- unsupported backend IR shapes in direct JIT lowering
- missing backend metadata resolution for newly introduced declaration shapes
