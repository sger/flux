# Proposal 018: Runtime and Language Evolution Roadmap

**Status:** Active
**Priority:** Meta (Planning)
**Created:** 2026-02-08
**Related:** Proposal 019 (Zero-Copy Value Passing), Proposal 016 (Tail-Call Optimization), Proposal 017 (Persistent Collections and GC)
**Implementation Order:** 019 → 016 → 017 → 018 (this)

---

## Current State Snapshot

### What Flux Has Today

| Area | Summary |
|------|---------|
| **Expressions** | 20 variants (literals, if/else, match, call, lambda, pipe, array, hash, Some/Left/Right) |
| **Statements** | 7 variants (let, assign, return, expression, function, module, import) |
| **Patterns** | 7 variants (wildcard, literal, identifier, None, Some, Left, Right) + guards |
| **Operators** | 16 (arithmetic, comparison, logical, modulo, pipe, member access) |
| **Builtins** | 35 (array, string, hash, math, type-checking, I/O) |
| **Opcodes** | 44 (OpCode 0-43) |
| **Lexer** | Byte-dispatch, span-backed lexemes, symbol interning, string interpolation |
| **Parser** | Recursive descent + Pratt precedence, 3-token lookahead, error recovery |
| **Compiler** | 2-pass (predeclare functions, then compile), module constant evaluation |
| **VM** | Stack-based (2048 slots), 65536 globals, frame stack, 35 builtins |
| **Module System** | Module declarations, imports with aliases, cycle detection, topological ordering |
| **Diagnostics** | Structured errors (E101-E1011), source snippets, hints, suggestions, aggregation |
| **Linter** | 10 warnings (unused vars/params/imports, shadowing, naming, dead code, complexity) |
| **Formatter** | Basic indentation (4 spaces), whitespace normalization |
| **Bytecode Cache** | `.fxc` files in `target/flux/`, invalidated on source/compiler changes |
| **Tests** | 41 test files, snapshot testing (insta), Criterion benchmarks |

### What Flux Does NOT Have

| Gap | Impact |
|-----|--------|
| Tail-call optimization | Stack overflow on deep recursion (>2048 frames) |
| Constant folding | `5 + 3` compiled as two constants + OpAdd instead of `8` |
| Constant pool deduplication | Same `42` stored N times in constant pool instead of once |
| Peephole optimization | Redundant instructions not eliminated |
| Liveness analysis | Cannot determine when variables die (blocks Proposal 016 Phase 2) |
| Symbol interning in compiler | Lexer interns to `Symbol(u32)` but compiler re-hashes full Strings |
| Arena allocation | AST uses individual `Box` allocations, poor cache locality |
| Destructuring | `let [a, b] = pair` or `let {x, y} = point` not supported |
| For loops / ranges | No `for x in collection` or `1..10` syntax |
| Block comments | Only `//` line comments, no `/* */` |
| User-defined types (ADTs) | No `type Shape { Circle(r), Rect(w, h) }` |
| Tuples | No `(a, b)` type; arrays used instead |
| Type system | Fully dynamic; no static checking |
| Default parameters | `fun f(x, y = 0)` not supported |
| Array destructuring in patterns | `match arr { [a, b, ...rest] -> ... }` not supported |
| String escape builtins | No `char_at`, `starts_with`, `ends_with`, `replace`, `index_of` |
| Mid-level IR | AST compiled directly to bytecode; no optimization layer |
| LSP / Language Server | No IDE integration beyond the formatter |

---

## Prioritized Roadmap

Features are grouped into tiers based on **impact** (what it unlocks for users), **effort** (implementation complexity), and **dependencies** (what must come first).

---

### Tier 1: Performance Foundations (Highest Priority)

These are pure runtime/compiler improvements with no syntax changes. They make existing Flux programs faster and more capable.

#### 1.1 Tail-Call Elimination (Proposal 016, Phase 1)

**What:** Add `OpTailCall` opcode. Self-recursive calls in tail position reuse the current frame instead of pushing a new one.

**Why now:** Without this, any recursive algorithm deeper than ~2000 calls crashes. This blocks the standard library (Flow.List) from being usable on real data.

**Impact:** Unbounded recursion depth. `countdown(1_000_000)` works.

| Item | Detail |
|------|--------|
| New opcode | `OpTailCall = 44` (1-byte operand: arg count) |
| Compiler change | Add `in_tail_position: bool` flag, propagate through if/else/match |
| VM change | `tail_call_closure()` overwrites args + resets IP, no new frame |
| Effort | ~1 week |
| Dependencies | None |
| Risk | Low — only affects self-calls detected by `SymbolScope::Function` |

#### 1.2 Liveness Analysis in Symbol Table

**What:** Extend `Symbol` with `use_count: u16` and `last_use: Option<usize>` (bytecode offset). Track during compilation.

**Why now:** Foundation for Phase 2 (array reuse), dead code elimination, and better linter warnings. Without it, the compiler cannot determine when a variable is dead.

**Impact:** Enables `OpConsumeLocal`, better unused-variable detection at compile time.

| Item | Detail |
|------|--------|
| Symbol change | Add `use_count`, `last_use` fields |
| Compiler change | Increment `use_count` on every `load_symbol()`, record position |
| Effort | ~3-4 days |
| Dependencies | None |
| Risk | None — additive change, no behavior modification |

#### 1.3 Accumulator Array Reuse (Proposal 016, Phase 2)

**What:** Add `OpConsumeLocal` opcode. When the compiler proves a local is dead after a tail-call argument expression, it moves (instead of clones) the value. Combined with builtin ownership refactor, arrays mutate in-place.

**Why now:** The O(n^2) accumulator problem is the single biggest performance issue in Flux. `build(10000, [])` does ~50M element copies today.

**Impact:** O(n^2) -> O(n) for accumulator patterns. 100-1000x speedup.

| Item | Detail |
|------|--------|
| New opcode | `OpConsumeLocal = 45` (uses `std::mem::replace`) |
| Compiler change | Emit `OpConsumeLocal` when local is dead + not in `free_symbols` |
| Builtin change | `builtin_push`, `concat`, `reverse`, `sort` use `swap_remove` |
| Effort | ~1 week |
| Dependencies | 1.1 (TCE) + 1.2 (liveness) |
| Risk | Low — conservative analysis; only parameters, not captured |

#### 1.4 Constant Folding

**What:** Post-parse optimization pass that evaluates constant expressions at compile time.

**Why now:** Every `5 + 3` emits two `OpConstant` + one `OpAdd`. Folding turns it into one `OpConstant 8`. This is low-hanging fruit that improves both bytecode size and execution speed.

**Impact:** 10-20% bytecode size reduction, proportional speedup for arithmetic-heavy code.

| Item | Detail |
|------|--------|
| New pass | Fold `Infix`/`Prefix` on literal operands during compilation |
| Scope | Integer, float, string concat, boolean ops, comparisons |
| Effort | ~3-4 days |
| Dependencies | None |
| Risk | None — produces same result, fewer instructions |

#### 1.5 Constant Pool Deduplication

**What:** Before appending to the constant pool, check if an identical constant already exists. Return the existing index.

**Why now:** `add_constant()` currently appends blindly. The same integer `42` appearing 5 times creates 5 constant pool entries. This wastes bytecode space, cache lines, and makes the `.fxc` cache larger.

**Impact:** 20-40% constant pool size reduction for typical programs. Better cache behavior.

| Item | Detail |
|------|--------|
| Compiler change | Add `constant_map: HashMap<ConstKey, usize>` to `Compiler`; check before insert |
| Key type | Hash integers/floats/strings/booleans; skip non-hashable (arrays, closures) |
| Effort | ~half day |
| Dependencies | None |
| Risk | None — purely internal, same semantics |

```rust
// Before (current)
pub(super) fn add_constant(&mut self, obj: Object) -> usize {
    self.constants.push(obj);
    self.constants.len() - 1
}

// After
pub(super) fn add_constant(&mut self, obj: Object) -> usize {
    if let Some(key) = ConstKey::from_object(&obj) {
        if let Some(&idx) = self.constant_map.get(&key) {
            return idx;
        }
        let idx = self.constants.len();
        self.constant_map.insert(key, idx);
        self.constants.push(obj);
        idx
    } else {
        self.constants.push(obj);
        self.constants.len() - 1
    }
}
```

#### 1.6 Symbol Interning Through the Full Pipeline

**What:** Propagate the lexer's `Symbol(u32)` IDs through the AST and into the compiler's `SymbolTable`, replacing String-based lookups with u32 comparisons.

**Current state:** The lexer interns identifiers into `Symbol(u32)` via `Interner`. But the parser discards these IDs — the AST stores identifier names as `String`. The compiler's `SymbolTable` uses `HashMap<String, Symbol>`, re-hashing every identifier lookup from scratch.

**Why now:** Every variable reference in the compiler does a `HashMap<String, _>` lookup (hash + compare bytes). With u32 symbol IDs, this becomes a `HashMap<u32, _>` lookup (hash one u32 — effectively a single instruction). For identifier-heavy code, this is a significant portion of compilation time.

**Impact:** Faster compilation. Estimated 15-30% reduction in compiler symbol resolution time.

| Item | Detail |
|------|--------|
| AST change | Store `symbol: Symbol` alongside or instead of `name: String` in `Identifier` |
| Parser change | Pass `Interner` through parser; carry `Symbol` from token to AST |
| Compiler change | `SymbolTable.store: HashMap<Symbol, CompilerSymbol>` (u32 key) |
| Effort | ~1-2 weeks (touches many files) |
| Dependencies | Feature/byte branch merged (lexer interning) |
| Risk | Medium — wide-reaching change across AST/parser/compiler |

#### 1.7 Arena Allocation for AST Nodes

**What:** Use a typed arena allocator (`bumpalo` or `typed-arena`) for AST node allocation instead of individual `Box<Expression>` heap allocations.

**Current state:** Every `Expression::If`, `Expression::Call`, `Expression::Infix`, etc. allocates a separate `Box` on the heap. For a program with 10,000 expressions, this is 10,000 individual allocations scattered across memory.

**Why now:** Arena allocation groups all AST nodes into a single contiguous block. Allocation is a pointer bump (nearly free). Deallocation is a single `drop`. Cache locality is dramatically better since related nodes sit next to each other in memory.

**Impact:** Faster parsing (allocation cost near-zero), faster compilation (better cache locality during AST traversal), faster cleanup (single deallocation).

| Item | Detail |
|------|--------|
| New dependency | `bumpalo` or `typed-arena` in `Cargo.toml` |
| AST change | `Box<Expression>` → `&'arena Expression` (lifetime-bound references) |
| Parser change | Parser holds `&'arena Bump`, allocates nodes with `arena.alloc(...)` |
| Effort | ~2-3 weeks (lifetime annotations propagate widely) |
| Dependencies | None |
| Risk | Medium — lifetime annotations can be invasive; requires careful design |

#### 1.8 VM Dispatch Table

**What:** Replace the `match` in `dispatch_instruction()` and `OpCode::from(u8)` with an explicit function pointer table indexed by opcode byte.

**Current state:** `dispatch_instruction()` is a 200+ line match with 40+ arms. `OpCode::from(u8)` is another 44-arm match. LLVM likely compiles both to jump tables, but this is not guaranteed (especially for debug builds or when arms have varying complexity).

**Why now:** An explicit dispatch table is a guaranteed O(1) lookup, portable across optimization levels, and makes the dispatch cost predictable. It also enables future extensions like threaded dispatch (each handler jumps directly to the next).

**Impact:** Minor for release builds (LLVM already optimizes), significant for debug builds (~2-3x dispatch speedup). Enables future threaded/computed-goto dispatch.

| Item | Detail |
|------|--------|
| VM change | `static DISPATCH: [fn(&mut VM, usize) -> Result<bool, String>; 256]` |
| OpCode change | Replace `From<u8>` match with `unsafe { transmute }` (with bounds check) or lookup table |
| Effort | ~3-4 days |
| Dependencies | None |
| Risk | Low — behavioral equivalent; test suite validates |

```rust
// Before
let op = OpCode::from(instructions[ip]);
let advance = self.dispatch_instruction(ip, op)?;

// After
let byte = instructions[ip];
let advance = DISPATCH_TABLE[byte as usize](self, ip)?;
```

---

### Tier 1 Summary: Infrastructure Performance

| # | Item | Status | Effort | Impact |
|---|------|--------|--------|--------|
| 1.1 | Tail-Call Elimination | Not started | 1 week | Unbounded recursion |
| 1.2 | Liveness Analysis | Not started | 3-4 days | Foundation for 1.3 |
| 1.3 | Accumulator Array Reuse | Not started | 1 week | O(n^2) → O(n) |
| 1.4 | Constant Folding | Not started | 3-4 days | 10-20% bytecode reduction |
| 1.5 | Constant Pool Dedupe | Not started | half day | 20-40% pool reduction |
| 1.6 | Symbol Interning Pipeline | 50% (lexer done) | 1-2 weeks | 15-30% faster symbol resolution |
| 1.7 | Arena Allocation | Not started | 2-3 weeks | Faster parse + better cache |
| 1.8 | VM Dispatch Table | Partial (LLVM helps) | 3-4 days | Predictable dispatch, enables threading |

---

### Tier 2: Language Expressiveness (High Priority)

Features that make Flux programs more readable and idiomatic. Each one reduces boilerplate.

#### 2.1 Destructuring in Let Bindings

**What:** `let [a, b] = pair;` and `let {name, age} = person;`

**Why now:** Without destructuring, extracting values from arrays/hashes requires verbose indexing. Every functional language has this. It's the most frequently-needed syntax improvement.

**Impact:** Dramatically cleaner code for multi-value returns and data extraction.

```flux
// Today
let x = point[0];
let y = point[1];

// With destructuring
let [x, y] = point;
```

| Item | Detail |
|------|--------|
| Parser change | Extend `parse_let_statement()` to recognize `[` and `{` patterns |
| AST change | Reuse existing `Pattern` enum in `Let` statement |
| Compiler change | Emit index/key lookups + OpSetLocal for each binding |
| Effort | ~1 week |
| Dependencies | None |
| Risk | Low |

#### 2.2 Rest Patterns in Arrays

**What:** `[head, ...tail]` in both `let` destructuring and `match` patterns.

**Why now:** This is the idiomatic way to process collections recursively. Currently `first(arr)` + `rest(arr)` is required, which is verbose and clones the tail.

```flux
// Today
match arr {
    _ if len(arr) > 0 -> {
        let head = first(arr);
        let tail = rest(arr);
        process(head, tail);
    },
    _ -> "empty",
}

// With rest patterns
match arr {
    [head, ...tail] -> process(head, tail),
    [] -> "empty",
}
```

| Item | Detail |
|------|--------|
| New pattern | `Pattern::Rest { binding, span }` or `Pattern::Array { elements, rest }` |
| Parser change | In array pattern, recognize `...identifier` |
| Compiler change | Emit `OpConstant(len)` + `OpIndex` for elements, `slice` for rest |
| Effort | ~1 week |
| Dependencies | 2.1 (Destructuring) |
| Risk | Low |

#### 2.3 For Loops

**What:** `for x in collection { ... }` iterating over arrays, strings, and (future) ranges.

**Why now:** Recursive iteration with `first`/`rest` is the only option today. `for` is the standard imperative escape hatch that every language needs. Without it, simple tasks like "print each element" require a recursive helper.

```flux
for item in items {
    print(item);
}

// Desugars to index-based loop:
// let _i = 0; while _i < len(items) { let item = items[_i]; ...; _i = _i + 1; }
```

| Item | Detail |
|------|--------|
| New statement | `Statement::For { binding, iterable, body, span }` |
| New keyword | `for`, `in` |
| Compiler output | Desugar to counter-based loop with `OpJump`/`OpJumpNotTruthy` |
| Effort | ~1 week |
| Dependencies | None |
| Risk | Low — syntactic sugar over existing jump infrastructure |

#### 2.4 Block Comments

**What:** `/* ... */` with nesting support.

**Why now:** Only `//` comments exist. Block comments are essential for temporarily disabling code and multi-line documentation. Minimal effort, high quality-of-life impact.

```flux
/* This function is
   temporarily disabled */

/* Nested /* comments */ work */
```

| Item | Detail |
|------|--------|
| Lexer change | Track nesting depth for `/*`/`*/` pairs |
| Effort | ~2-3 hours |
| Dependencies | None |
| Risk | None |

#### 2.5 String Builtins (Missing Essentials)

**What:** Add `starts_with`, `ends_with`, `replace`, `index_of`, `contains` (for strings), `char_at`.

**Why now:** String processing is a fundamental task. The current set (`split`, `join`, `trim`, `upper`, `lower`, `chars`, `substring`) is missing common operations that users expect.

| Item | Detail |
|------|--------|
| New builtins | 6 functions wrapping Rust `str` methods |
| Effort | ~1 day |
| Dependencies | None |
| Risk | None |

---

### Tier 3: Type System & Data Modeling (Medium Priority)

Features that enable richer domain modeling.

#### 3.1 Algebraic Data Types (ADTs)

**What:** User-defined sum types with pattern matching.

**Why now:** `Some`/`None` and `Left`/`Right` are hard-coded today. ADTs let users define their own domain types. This is the most impactful language-level feature for a functional language.

```flux
type Shape {
    Circle(radius)
    Rectangle(width, height)
    Triangle(a, b, c)
}

match shape {
    Circle(r) -> 3.14 * r * r,
    Rectangle(w, h) -> w * h,
    Triangle(a, b, c) -> heron(a, b, c),
}
```

| Item | Detail |
|------|--------|
| New statement | `Statement::TypeDecl { name, variants, span }` |
| New keyword | `type` |
| Object change | `Object::Variant { tag: u16, fields: Vec<Object> }` or tag + constant pool |
| Compiler change | Register constructors as functions, emit tag-based dispatch for patterns |
| Pattern change | `Pattern::Constructor { name, fields, span }` |
| Effort | ~2-3 weeks |
| Dependencies | None (but benefits from 3.2) |
| Risk | Medium — design decisions around generics, serialization |

#### 3.2 Record Types (Structs)

**What:** Named fields with construction and dot-access.

**Why now:** Hashes are the only way to model structured data today. Records provide named fields, construction validation, and better error messages.

```flux
type Point { x, y }

let p = Point { x: 10, y: 20 };
let moved = { p | x: p.x + 1 };   // functional update
```

| Item | Detail |
|------|--------|
| Representation | Could share `Object::Hash` or a dedicated `Object::Record` |
| Parser change | `type Name { field, field }` syntax |
| Access | Dot notation (already exists for modules) |
| Effort | ~2 weeks |
| Dependencies | 3.1 (ADTs share the `type` keyword) |
| Risk | Medium — overlap with hash literals |

#### 3.3 Tuples

**What:** Fixed-size, heterogeneous collections with `(a, b)` syntax.

**Why now:** Currently arrays serve as tuples, but they lack positional access (`pair.0`, `pair.1`) and carry the wrong semantics (variable-length vs fixed-length).

```flux
let pair = (1, "hello");
let (x, y) = pair;
```

| Item | Detail |
|------|--------|
| Object change | `Object::Tuple(Vec<Object>)` |
| Parser change | Parenthesized comma-separated expressions |
| Effort | ~1 week |
| Dependencies | 2.1 (Destructuring) |
| Risk | Low — parser must distinguish `(expr)` grouping from `(a, b)` tuple |

#### 3.4 Range Type

**What:** `1..10` (exclusive) and `1..=10` (inclusive) range syntax.

**Why now:** Enables `for i in 0..n` loops. Without ranges, users must use recursive counters.

```flux
for i in 0..10 {
    print(i);
}

let slice = arr[2..5];
```

| Item | Detail |
|------|--------|
| Object change | `Object::Range { start, end, inclusive }` |
| New tokens | `DotDot` (`..`), `DotDotEq` (`..=`) |
| Effort | ~1 week |
| Dependencies | 2.3 (For loops) |
| Risk | Low |

---

### Tier 4: Compiler Internals & Tooling (Medium Priority)

Improvements that don't change the language surface but improve developer experience and compiler quality.

#### 4.1 Peephole Optimization Pass

**What:** Post-compilation bytecode walk that eliminates redundant instructions.

**Patterns to optimize:**
- `OpPop` after `OpPop` -> single `OpPop`
- `OpJump` to next instruction -> remove
- `OpConstant(true); OpJumpNotTruthy` -> remove both
- `OpGetLocal N; OpPop` -> remove both (dead load)

**Why now:** The infrastructure exists (`last_instruction`, `previous_instruction` in `CompilationScope`). Low effort, measurable improvement.

| Item | Detail |
|------|--------|
| New pass | Walk bytecode after compilation, pattern-match and eliminate |
| Effort | ~1 week |
| Dependencies | None |
| Risk | Low — each pattern independently testable |

#### 4.2 Default Parameters

**What:** `fun f(x, y = 0) { ... }` — parameters with default values.

**Why now:** Reduces function overloading needs. Common in modern languages.

```flux
fun greet(name, greeting = "Hello") {
    "#{greeting}, #{name}!";
}

greet("World");          // "Hello, World!"
greet("World", "Hi");   // "Hi, World!"
```

| Item | Detail |
|------|--------|
| AST change | `Parameter { name, default: Option<Expression> }` |
| Compiler change | Emit default-filling prologue when args < params |
| Effort | ~3-4 days |
| Dependencies | None |
| Risk | Low |

#### 4.3 LSP Foundation

**What:** Language Server Protocol implementation for IDE features (diagnostics, go-to-definition, hover).

**Why now:** The diagnostic system, linter, and formatter already exist. Exposing them via LSP makes Flux usable in VS Code, Neovim, etc.

| Item | Detail |
|------|--------|
| New crate | `flux-lsp` wrapping existing syntax module |
| Features | Diagnostics on save, go-to-definition (symbol table), hover (type info) |
| Effort | ~3-4 weeks |
| Dependencies | None (builds on existing infrastructure) |
| Risk | Medium — LSP protocol complexity |

#### 4.4 Improved Formatter

**What:** Extend the current 79-line formatter with proper AST-based formatting.

**Why now:** The current formatter only handles indentation. A proper formatter should handle line breaks, alignment, trailing commas, and consistent spacing.

| Item | Detail |
|------|--------|
| Approach | AST-based prettyprinter (visit AST, emit formatted output) |
| Effort | ~2 weeks |
| Dependencies | None |
| Risk | Low |

---

### Tier 5: Advanced Features (Future)

These are substantial features that require significant design work and should be considered after Tiers 1-3 are stable.

| Feature | Description | Effort | Dependencies |
|---------|-------------|--------|--------------|
| **List comprehensions** | `[x * 2 for x in arr if x > 0]` | 2 weeks | For loops, arrays |
| **Type inference** | Hindley-Milner style type checking | 6-8 weeks | ADTs, records |
| **Effect system** | `fun f() with IO { ... }` | 8-10 weeks | Type system |
| **Persistent collections** | Rc-based cons list + HAMT (Proposal 017 revised) | 4-6 weeks | TCE |
| **Concurrency (actors)** | `spawn`, message passing, supervision | 8-12 weeks | Effect system |
| **Package manager** | Dependency resolution, versioned modules | 6-8 weeks | Module system |
| **REPL** | Interactive read-eval-print loop | 2-3 weeks | None |
| **Debugger** | Step-through execution, breakpoints | 4-6 weeks | LSP, source maps |

---

## Recommended Implementation Order

```
Week 1-2:   1.1 Tail-Call Elimination
            1.2 Liveness Analysis
            1.5 Constant Pool Deduplication
            2.4 Block Comments
            2.5 String Builtins
                                          ← Ship: v0.2.0 (recursion + quick wins)

Week 3-4:   1.3 Accumulator Array Reuse
            1.4 Constant Folding
            1.8 VM Dispatch Table
            2.3 For Loops
                                          ← Ship: v0.3.0 (performance + iteration)

Week 5-7:   1.6 Symbol Interning Pipeline
            2.1 Destructuring
            2.2 Rest Patterns
            3.3 Tuples
            3.4 Ranges
                                          ← Ship: v0.4.0 (interning + destructuring)

Week 8-10:  1.7 Arena Allocation
            3.1 ADTs
            3.2 Records
            4.1 Peephole Optimization
                                          ← Ship: v0.5.0 (user-defined types + arena)

Week 11-14: 4.2 Default Parameters
            4.3 LSP Foundation
            4.4 Improved Formatter
                                          ← Ship: v0.6.0 (tooling)

Week 15+:   Tier 5 features based on user feedback
```

---

## Interaction with Existing Proposals

| Proposal | Roadmap Item | Notes |
|----------|-------------|-------|
| 016 (Tail-Call) | 1.1 + 1.3 | Implement as-is. Phase 1 first, Phase 2 after liveness. |
| 017 (Persistent Collections) | Tier 5 | Revise to use Rc-based sharing (no GC needed). Defer until after TCE proves itself. |
| 010 (GC) | Deferred | Not needed with Rc-based approach. Revisit only if mutable refs or lazy eval added. |
| 003 (Stdlib/Flow) | 2.5 + Tier 5 | String builtins now. Flow modules after for-loops + TCE make them practical. |
| 004 (Language Features) | 2.1-2.4, 3.1-3.4 | Many items already implemented (operators, pipe, lambda, guards). Remaining items mapped above. |
| 005 (Symbol Interning) | In progress | Partially implemented on `feature/byte` branch (lexer-level). Extend to compiler. |
| 007 (Visitor Pattern) | Tier 4 | Useful for multi-pass analysis. Defer until type system work begins. |
| 009 (Macros) | Tier 5 | Defer. Language surface must stabilize first. |
| 015 (Package/Module MVP) | Tier 5 | Defer until module system is battle-tested. |

---

## Decision Points

These design questions should be resolved before implementation begins:

1. **Destructuring syntax**: `let [a, b] = x` (array) vs `let (a, b) = x` (tuple) — if tuples exist, both needed?

2. **For-loop desugaring**: Counter-based (preserves array semantics) vs iterator-protocol (future-proof but complex)?

3. **ADT representation**: Tagged union in `Object` enum (fast, requires variant) vs hash-table encoding (flexible, slower)?

4. **Record vs Hash overlap**: Should `{name: "Alice", age: 30}` be a hash or a record? Or should records use different syntax (`Point { x: 1, y: 2 }`)?

5. **Range semantics**: Lazy (generates values on demand) vs eager (creates array)? Lazy is better but needs iterator protocol.

6. **Type annotation syntax**: `fun f(x: Int) -> String` or inferred-only? Adding annotations early constrains future type system design.

---

## Metrics to Track

| Metric | Current | Target |
|--------|---------|--------|
| Max recursion depth | ~2048 | Unbounded (after 1.1) |
| `build(10000, [])` time | O(n^2) ~seconds | O(n) ~milliseconds (after 1.3) |
| Bytecode size (avg program) | Baseline | -15% (after 1.4 + 4.1) |
| Linter warnings | 10 types | 15+ types (after liveness) |
| Builtin count | 35 | 41+ (after 2.5) |
| Test files | 41 | 55+ (after each tier) |
