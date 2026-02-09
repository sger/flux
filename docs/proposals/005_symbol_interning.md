# Proposal 005: Symbol Interning

**Status:** Draft (Phase 3)
**Author:** @sgerokostas
**Created:** 2026-02-01
**Related:** Module Constants (v0.0.2), Phase 3 optimization
**Priority:** Low (defer until profiling justifies)

---

## Summary

Replace string-based identifiers with interned symbols (u32 IDs) throughout the compiler to reduce memory usage by 70-80% and improve performance by 2-3x for identifier operations.

---

## Motivation

### Current Problem

Identifiers are stored as `String` throughout the codebase:

```rust
// AST nodes
Expression::Identifier { name: String, ... }

// Compiler state
HashMap<String, Object>           // module_constants
HashMap<String, Symbol>           // symbol_table

// Module constants
HashMap<String, Vec<String>>      // dependencies
```

**Costs:**
- **Memory:** 24 bytes per String (8 byte pointer + 8 length + 8 capacity)
- **Cloning:** Frequent clones for HashMap keys (~150 clones for 50 constants)
- **Comparison:** String comparison via `memcmp` (slower than integer compare)
- **Hashing:** Hash entire string bytes (slower than hash u32)

**For a module with 50 constants:**
- String clones: ~150 × 20 bytes = **3 KB**
- Hash operations: ~500 × hash(~10 chars) = **10-50μs**
- Total overhead: **~15-55μs per module**

**Current status:** Not a bottleneck (< 0.1% of compile time for typical modules)

---

## Proposed Design

### Overview

Convert strings to integers once during parsing, use integers everywhere:

```rust
// Before
"counter" → hash("counter") → compare chars → 24 bytes

// After
"counter" → 42 (u32) → compare ints → 4 bytes
```

---

## Implementation

### 1. Symbol Interner

```rust
// src/syntax/interner.rs

use std::collections::HashMap;

/// Compact symbol identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SymbolId(u32);

impl SymbolId {
    pub const NONE: Self = SymbolId(u32::MAX);
}

/// Global symbol table that interns strings to unique IDs
pub struct SymbolInterner {
    /// All unique strings (append-only, never removed)
    strings: Vec<String>,
    /// Fast lookup: string → ID
    lookup: HashMap<String, SymbolId>,
}

impl SymbolInterner {
    /// Create a new empty interner
    pub fn new() -> Self {
        Self {
            strings: Vec::new(),
            lookup: HashMap::new(),
        }
    }

    /// Intern a string, returning its unique ID
    ///
    /// If the string already exists, returns existing ID.
    /// Otherwise, allocates new ID and stores string.
    pub fn intern(&mut self, s: &str) -> SymbolId {
        if let Some(&id) = self.lookup.get(s) {
            return id;
        }

        let id = SymbolId(self.strings.len() as u32);
        self.strings.push(s.to_string());
        self.lookup.insert(s.to_string(), id);
        id
    }

    /// Resolve a symbol ID back to its string
    pub fn resolve(&self, id: SymbolId) -> &str {
        &self.strings[id.0 as usize]
    }

    /// Get number of interned symbols
    pub fn len(&self) -> usize {
        self.strings.len()
    }
}
```

### 2. Update AST

```rust
// src/syntax/expression.rs

// Before
pub enum Expression {
    Identifier {
        name: String,
        span: Span,
    },
    // ...
}

// After
pub enum Expression {
    Identifier {
        name: SymbolId,
        span: Span,
    },
    // ...
}
```

### 3. Update Parser

```rust
// src/syntax/parser.rs

pub struct Parser {
    lexer: Lexer,
    interner: SymbolInterner,  // NEW
    // ... existing fields
}

impl Parser {
    pub fn new(lexer: Lexer) -> Self {
        Self {
            lexer,
            interner: SymbolInterner::new(),
            // ... existing fields
        }
    }

    fn parse_identifier(&mut self) -> SymbolId {
        let name = &self.current_token.literal;
        self.interner.intern(name)
    }

    pub fn into_parts(self) -> (Program, SymbolInterner) {
        (self.program, self.interner)
    }
}
```

### 4. Update Compiler

```rust
// src/bytecode/compiler.rs

pub struct Compiler {
    interner: &'ctx SymbolInterner,
    module_constants: HashMap<SymbolId, Object>,
    symbol_table: SymbolTable<SymbolId>,
    // ... existing fields
}

impl Compiler {
    pub fn new(interner: &'ctx SymbolInterner) -> Self {
        Self {
            interner,
            module_constants: HashMap::new(),
            // ... existing fields
        }
    }
}
```

### 5. Update Module Constants

```rust
// src/bytecode/module_constants/analysis.rs

pub struct ModuleConstantAnalysis<'a> {
    pub eval_order: Vec<SymbolId>,
    pub expressions: HashMap<SymbolId, (&'a Expression, Position)>,
}

// src/bytecode/module_constants/eval.rs

pub fn eval_const_expr(
    expr: &Expression,
    defined: &HashMap<SymbolId, Object>,
    interner: &SymbolInterner,  // For error messages
) -> Result<Object, ConstEvalError> {
    match expr {
        Expression::Identifier { name, .. } => {
            defined.get(name).cloned().ok_or_else(|| {
                let name_str = interner.resolve(*name);
                ConstEvalError::new("E041", format!("'{}' is not a module constant.", name_str))
            })
        }
        // ... rest unchanged
    }
}
```

---

## Performance Analysis

### Memory Savings

| Metric | String | SymbolId | Savings |
|--------|--------|----------|---------|
| **Size per identifier** | 24 bytes | 4 bytes | **83%** |
| **1000 identifiers** | 24 KB | 4 KB | **20 KB** |
| **Duplicate storage** | Each occurrence | Once in interner | Variable |

**Example: 1000 identifiers with 100 unique strings (10 chars avg)**
- **Before:** 1000 × 24 bytes = 24 KB
- **After:** 100 × (10 + 24) + 1000 × 4 = 7.4 KB
- **Savings:** 70% (16.6 KB)

### Speed Improvements

| Operation | String | SymbolId | Speedup |
|-----------|--------|----------|---------|
| **Clone** | memcpy(~10 bytes) | copy 4 bytes | **2-3x** |
| **Equality** | memcmp(~10 bytes) | int compare | **5-10x** |
| **Hash** | hash(~10 bytes) | hash 4 bytes | **3-5x** |
| **HashMap lookup** | hash + memcmp | hash + int eq | **2-4x** |

**Overall:** 2-3x faster for identifier-heavy operations

---

## Implementation Phases

### Phase 1: Minimal (Module Constants Only)

**Scope:** Intern only constant names during analysis

```rust
// src/bytecode/module_constants/interner.rs
pub struct ConstantInterner { ... }

let analysis = analyze_module_constants_interned(body, &mut interner)?;
```

**Effort:** 1-2 days
**Pros:** Isolated, low risk, measurable impact
**Cons:** Partial solution, doesn't benefit rest of compiler

### Phase 2: Parser-Level (Recommended)

**Scope:** Intern all identifiers during parsing

**Files to update:**
- `src/syntax/interner.rs` (new)
- `src/syntax/parser.rs` (~50 lines changed)
- `src/syntax/expression.rs` (~10 lines changed)
- `src/syntax/statement.rs` (~20 lines changed)
- `src/bytecode/compiler.rs` (~100 lines changed)
- `src/bytecode/module_constants/*.rs` (~50 lines changed)

**Effort:** 1-2 weeks
**Pros:** Global benefit, clean design
**Cons:** Larger refactor, lifetime management

### Phase 3: Full (With Arena Allocation)

**Scope:** Add arena allocator for temporary objects

```rust
pub struct Compiler<'ctx> {
    interner: &'ctx SymbolInterner,
    arena: &'ctx Arena,  // Bump allocator
    // ...
}
```

**Effort:** 2-3 weeks
**Pros:** Maximum performance
**Cons:** Complex, significant API changes

---

## Trade-offs

### ✅ Benefits

1. **Memory:** 70-80% reduction for identifier storage
2. **Speed:** 2-3x faster comparisons and hashing
3. **Cache efficiency:** Better locality (dense integers)
4. **Equality:** O(1) comparison (int == int)
5. **Deduplication:** Each unique string stored once

### ❌ Costs

1. **Complexity:** Interner must be threaded through code
2. **Lifetimes:** Interner must outlive all SymbolIds
3. **Debuggability:** Must resolve IDs to see string names
4. **Initial cost:** Interning requires hash + insert (one-time)
5. **Memory:** Interner overhead (~100 KB for 5000 unique strings)

---

## When to Implement

### ❌ Don't Implement Now If:

- Compile time < 1 second for typical files
- Typical files have < 1000 identifiers
- No profiling data showing identifier operations as bottleneck
- Other features have higher priority

### ✅ Implement When:

1. **Profiling shows:** String operations consume >5% of compile time
2. **Large files:** Files with >10K identifiers are common
3. **IDE integration:** Need fast identifier lookups for autocomplete/hover
4. **Phase 3 work:** Doing major optimization pass

---

## Migration Strategy

### Step 1: Measure Baseline
```bash
cargo build --release
hyperfine 'target/release/flux examples/large_file.flx'
perf record -g target/release/flux examples/large_file.flx
perf report
```

### Step 2: Implement Phase 2
- Add `SymbolInterner` to parser
- Update AST to use `SymbolId`
- Update compiler to accept interner
- Update module constants

### Step 3: Measure Impact
```bash
cargo build --release
hyperfine 'target/release/flux examples/large_file.flx'
# Compare with baseline
```

### Step 4: Evaluate
- If speedup < 10%, may not be worth complexity
- If speedup > 20%, proceed with full rollout
- If 10-20%, depends on other factors (IDE, memory)

---

## Alternative Approaches

### Option A: Rc<str> (Shared String References)

```rust
HashMap<Rc<str>, Object>
```

**Pros:** Cheap clones, easier than lifetimes
**Cons:** Reference counting overhead, not as fast as u32

### Option B: Cow<'static, str> (Copy-on-Write)

```rust
HashMap<Cow<'static, str>, Object>
```

**Pros:** Zero-copy for literals
**Cons:** Still stores String for non-literals

### Option C: String Interning Libraries

- `string-interner` crate (mature, well-tested)
- `lasso` crate (thread-safe, fast)

**Pros:** Battle-tested, feature-rich
**Cons:** External dependency, less control

**Recommendation:** Use `lasso` if implementing Phase 2+

---

## Testing Strategy

### 1. Unit Tests

```rust
#[test]
fn test_interner_deduplication() {
    let mut interner = SymbolInterner::new();
    let id1 = interner.intern("foo");
    let id2 = interner.intern("foo");
    assert_eq!(id1, id2);
    assert_eq!(interner.len(), 1);
}

#[test]
fn test_interner_uniqueness() {
    let mut interner = SymbolInterner::new();
    let id1 = interner.intern("foo");
    let id2 = interner.intern("bar");
    assert_ne!(id1, id2);
}
```

### 2. Integration Tests

Ensure all existing tests pass with symbol IDs:
- Module constants tests
- Parser tests
- Compiler tests

### 3. Benchmarks

```rust
#[bench]
fn bench_string_lookup(b: &mut Bencher) {
    let map: HashMap<String, i32> = ...;
    b.iter(|| map.get("identifier"));
}

#[bench]
fn bench_symbol_lookup(b: &mut Bencher) {
    let map: HashMap<SymbolId, i32> = ...;
    b.iter(|| map.get(&SymbolId(42)));
}
```

---

## Documentation Updates

1. **ARCHITECTURE.md** - Add section on symbol interning
2. **CONTRIBUTING.md** - Explain how to use interner
3. **API docs** - Document `SymbolId` and `SymbolInterner`

---

## Rollout Plan

### v0.0.2-v0.0.4 (Current)
- ❌ **Do not implement** - focus on Phase 1 module split
- ✅ **Document** - this proposal

### v0.1.0 (Phase 3 - Optimization)
- ✅ **Profile** - measure baseline performance
- ✅ **Decide** - implement if justified by profiling
- ✅ **Benchmark** - verify improvements

### Future (If Needed)
- Implement Phase 2 (parser-level interning)
- Evaluate Phase 3 (arena allocation)

---

## Success Criteria

**Phase 2 implementation is successful if:**
1. ✅ All existing tests pass
2. ✅ Compile time improves by >10% for large files
3. ✅ Memory usage reduced by >50% for identifiers
4. ✅ No regressions in code quality or maintainability

---

## References

1. **Papers:**
   - ["String Interning" - Wikipedia](https://en.wikipedia.org/wiki/String_interning)

2. **Implementations:**
   - [lasso crate](https://crates.io/crates/lasso) - Production-ready Rust interner
   - [rustc source](https://github.com/rust-lang/rust/tree/master/compiler/rustc_span) - Symbol interning in Rust compiler

3. **Related:**
   - [Proposal 001: Module Constants](./001_module_constants.md)
   - [COMPILER_ARCHITECTURE.md](../COMPILER_ARCHITECTURE.md) - Phase 3 overview

---

## Decision

**Status:** Draft - defer until Phase 3
**Rationale:** Current performance is acceptable. Focus on maintainability (Phase 1) before optimization (Phase 3).

**Next steps:**
1. Complete Phase 1 (module split)
2. Add performance benchmarks
3. Profile compiler on large files (>5000 identifiers)
4. Revisit this proposal in v0.1.0
