# Proposal 011: Phase 2 - Module System Enhancements

**Status:** Planning
**Priority:** High (Developer Experience)
**Created:** 2026-02-04
**Depends on:** Phase 1 Module Split (Proposal 006) ✅

## Overview

This proposal outlines comprehensive module system improvements for Phase 2. Building on Phase 1's code organization success, Phase 2 focuses on **module functionality** - adding selective imports, re-exports, explicit visibility, package management foundations, and standard library infrastructure.

## Problem Statement

### Current Module System Limitations

**What Works Today (✅):**
- Module declaration and nested namespaces
- Qualified imports with aliases
- Privacy via `_` prefix convention
- Cycle detection and topological ordering
- Per-module bytecode caching
- Multiple module roots

**What's Missing (❌):**
1. **Selective imports** - Must import entire module, can't select specific functions
2. **Re-exports** - Can't expose imported modules through current module
3. **Explicit visibility** - Only `_` prefix, no fine-grained control
4. **Wildcard imports** - No convenient way to import multiple items
5. **Module metadata** - No version info, documentation, or attributes
6. **Standard library** - No official stdlib modules shipped with Flux
7. **Package system** - No way to distribute/manage third-party modules
8. **Incremental compilation** - Cache doesn't leverage dependency graph fully
9. **Module documentation** - No docstrings or generated docs
10. **Module testing** - No built-in test framework for modules

### Impact

**For Users:**
- ❌ Can't write `import Math { square, cube }` - pollutes namespace
- ❌ Can't build modular libraries with re-exports
- ❌ No standard library for common tasks (List, Option, Result)
- ❌ Manual dependency management (no package manager)
- ❌ Slow rebuilds (doesn't skip unchanged modules)

**For Language Growth:**
- ❌ Can't ship official standard library
- ❌ No ecosystem for third-party packages
- ❌ Hard to maintain large codebases without selective imports

---

## Scope

### In Scope (Phase 2)

**Priority 1 (HIGH) - Core Language Features:**
1. ✅ Selective imports (`import Foo { bar, baz }`)
2. ✅ Re-exports (`export { bar } from Foo`)
3. ✅ Explicit visibility (`pub fun`, `pub let`)
4. ✅ Wildcard imports (`import Foo.*`)
5. ✅ Module documentation (docstrings)

**Priority 2 (MEDIUM) - Standard Library:**
6. ✅ Official `Flow.List` module (map, filter, reduce, etc.)
7. ✅ Official `Flow.Option` module (unwrap_or, map, and_then)
8. ✅ Official `Flow.Result` module (Either/Result helpers)
9. ✅ Official `Flow.String` module (split, join, trim, etc.)
10. ✅ Module metadata system (version, author, description)

**Priority 3 (LOW) - Tooling:**
11. ✅ Incremental compilation (skip unchanged modules)
12. ✅ Module documentation generator
13. ✅ Package manifest format (`flux.toml`)
14. ✅ Basic package resolution (local packages only)

### Out of Scope

- ❌ Remote package registry (defer to Phase 4)
- ❌ Semantic versioning / dependency resolution
- ❌ Module hot reloading
- ❌ FFI / external modules
- ❌ Module macros / code generation

---

## Detailed Plan

## 1. Selective Imports (HIGH PRIORITY)

### Current Behavior
```flux
import Math

// Must use qualified access for everything
Math.square(5);
Math.cube(10);
```

**Problem:** Pollutes scope with entire module, verbose for frequently used functions.

### Proposed Syntax

**Option A: Explicit List (Recommended)**
```flux
import Math { square, cube }

// Use directly without qualification
square(5);
cube(10);

// Rest of module still requires qualification
Math.add(1, 2);  // Error: add not imported
```

**Option B: Wildcard Import**
```flux
import Math.*

// Everything imported directly
square(5);
cube(10);
add(1, 2);
```

**Option C: Mixed**
```flux
import Math { square, cube, * }  // Square and cube explicit, rest wildcard
```

### Implementation

**1a. Update AST** ([src/frontend/statement.rs](src/frontend/statement.rs))
```rust
#[derive(Debug, Clone, PartialEq)]
pub enum ImportSpecifier {
    /// Import entire module: `import Math`
    Namespace,

    /// Import specific items: `import Math { square, cube }`
    Named(Vec<String>),

    /// Import all items: `import Math.*`
    Wildcard,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportStatement {
    pub module_path: Vec<String>,
    pub alias: Option<String>,
    pub specifier: ImportSpecifier,  // NEW
    pub position: Position,
}
```

**1b. Update Parser** ([src/frontend/parser/statement.rs](src/frontend/parser/statement.rs))
```rust
fn parse_import_statement(&mut self) -> Result<Statement, Box<Diagnostic>> {
    // ... existing module path parsing ...

    let specifier = if self.is_peek_token(&Token::LBrace) {
        self.next_token(); // consume {
        let mut names = Vec::new();

        while !self.is_peek_token(&Token::RBrace) {
            self.next_token();
            if let Token::Ident { literal, .. } = &self.current_token {
                names.push(literal.clone());
            }

            if self.is_peek_token(&Token::Comma) {
                self.next_token(); // consume comma
            }
        }

        self.expect_peek(Token::RBrace)?;
        ImportSpecifier::Named(names)
    } else if self.is_peek_token(&Token::Dot) {
        self.next_token(); // consume .
        self.expect_peek(Token::Asterisk)?;
        ImportSpecifier::Wildcard
    } else {
        ImportSpecifier::Namespace
    };

    // ... rest of import statement construction ...
}
```

**1c. Update Compiler** ([src/bytecode/compiler/statement.rs](src/bytecode/compiler/statement.rs))
```rust
fn compile_import_statement(&mut self, stmt: &ImportStatement) -> Result<()> {
    match &stmt.specifier {
        ImportSpecifier::Namespace => {
            // Existing behavior - bind module namespace
            self.bind_module_namespace(stmt)?;
        }
        ImportSpecifier::Named(names) => {
            // New behavior - bind specific exports
            for name in names {
                self.bind_imported_symbol(&stmt.module_path, name, stmt.position)?;
            }
        }
        ImportSpecifier::Wildcard => {
            // Import all public members
            self.bind_all_module_exports(&stmt.module_path, stmt.position)?;
        }
    }
    Ok(())
}

fn bind_imported_symbol(
    &mut self,
    module_path: &[String],
    symbol_name: &str,
    position: Position,
) -> Result<()> {
    // Check if module exports this symbol
    let module_node = self.get_module_node(module_path)?;

    if !module_node.exports_symbol(symbol_name) {
        return Err(diagnostic!(
            IMPORT_NOT_FOUND,
            "Module `{}` does not export `{}`",
            module_path.join("."),
            symbol_name
        ));
    }

    // Check if it's private
    if symbol_name.starts_with('_') {
        return Err(make_private_member_error(symbol_name, position));
    }

    // Bind to local scope
    self.symbol_table.define_global(symbol_name);
    Ok(())
}
```

**Estimated Effort:** 3-4 days

---

## 2. Re-exports (HIGH PRIORITY)

### Current Behavior
```flux
// lib/utils.flx
module Utils {
  // Can't expose Math.square as Utils.square
}
```

**Problem:** Can't build facade modules or aggregate exports.

### Proposed Syntax

**Option A: Export Declaration (Recommended)**
```flux
module Utils {
  import Math { square, cube }

  export { square, cube }  // Re-export imported symbols

  pub fun double(x) { x * 2 }
}

// Usage
import Utils { square, double }
```

**Option B: Direct Re-export**
```flux
module Utils {
  export { square, cube } from Math
}
```

**Option C: Export All**
```flux
module Utils {
  export * from Math
}
```

### Implementation

**2a. Add Export Statement** ([src/frontend/statement.rs](src/frontend/statement.rs))
```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    // ... existing variants ...
    Export {
        specifier: ExportSpecifier,
        position: Position,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExportSpecifier {
    /// Re-export specific symbols: `export { foo, bar }`
    Named(Vec<String>),

    /// Re-export from module: `export { foo } from Bar`
    NamedFrom {
        names: Vec<String>,
        module_path: Vec<String>,
    },

    /// Re-export all: `export * from Bar`
    Wildcard(Vec<String>),  // module_path
}
```

**2b. Module Export Tracking**
```rust
// In module_graph/mod.rs
#[derive(Debug, Clone)]
pub struct ModuleNode {
    pub id: ModuleId,
    pub path: PathBuf,
    pub program: Program,
    pub imports: Vec<ImportEdge>,
    pub exports: ModuleExports,  // NEW
}

#[derive(Debug, Clone)]
pub struct ModuleExports {
    /// Explicitly exported symbols
    pub explicit: HashSet<String>,

    /// Re-exported symbols from other modules
    pub reexports: HashMap<String, (ModuleId, String)>,

    /// All public symbols (implicit exports via pub)
    pub public: HashSet<String>,
}

impl ModuleNode {
    pub fn exports_symbol(&self, name: &str) -> bool {
        self.exports.explicit.contains(name)
            || self.exports.reexports.contains_key(name)
            || (self.exports.public.contains(name) && !name.starts_with('_'))
    }
}
```

**Estimated Effort:** 4-5 days

---

## 3. Explicit Visibility (HIGH PRIORITY)

### Current Behavior
```flux
module Math {
  fun square(x) { x * x }      // Public (implicit)
  fun _helper(x) { x + 1 }     // Private (by convention)
}
```

**Problem:**
- Only convention, not enforced
- Can't make `let` bindings public/private explicitly
- No way to document intent clearly

### Proposed Syntax

**Option A: Rust-style `pub` Keyword (Recommended)**
```flux
module Math {
  pub fun square(x) { x * x }     // Explicit public
  fun helper(x) { x + 1 }         // Private by default

  pub let PI = 3.14159            // Public constant
  let EPSILON = 0.0001            // Private constant
}
```

**Option B: Keep `_` prefix, add `pub` for constants**
```flux
module Math {
  fun square(x) { x * x }         // Public (backward compatible)
  fun _helper(x) { x + 1 }        // Private

  pub let PI = 3.14159            // Public constant (new)
  let _EPSILON = 0.0001           // Private constant
}
```

**Recommendation:** Option A for consistency, but provide migration guide.

### Implementation

**3a. Add Visibility to AST**
```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Visibility {
    Public,
    Private,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionStatement {
    pub visibility: Visibility,  // NEW
    pub name: String,
    pub parameters: Vec<String>,
    pub body: Block,
    pub position: Position,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LetStatement {
    pub visibility: Visibility,  // NEW
    pub name: String,
    pub value: Expression,
    pub position: Position,
}
```

**3b. Update Parser**
```rust
fn parse_function_statement(&mut self) -> Result<Statement, Box<Diagnostic>> {
    let visibility = if self.current_token_is(&Token::Pub) {
        self.next_token(); // consume 'pub'
        Visibility::Public
    } else {
        // Default: public for backward compatibility
        // OR: Visibility::Private (breaking change)
        Visibility::Public
    };

    self.expect_current(Token::Fun)?;
    // ... rest of function parsing ...
}
```

**3c. Module Export Analysis**
```rust
// During module graph construction
for stmt in &module.program.statements {
    match stmt {
        Statement::Function { visibility, name, .. } => {
            if *visibility == Visibility::Public {
                module_exports.public.insert(name.clone());
            }
        }
        Statement::Let { visibility, name, .. } => {
            if *visibility == Visibility::Public {
                module_exports.public.insert(name.clone());
            }
        }
        _ => {}
    }
}
```

**Migration Strategy:**
1. Phase 2a: Add `pub` keyword, default to public (backward compatible)
2. Phase 2b: Deprecation warnings for unprefixed private functions
3. Phase 3: Change default to private (breaking change, v0.2.0)

**Estimated Effort:** 2-3 days

---

## 4. Wildcard Imports (MEDIUM PRIORITY)

### Proposed Syntax
```flux
import Math.*

// Use all public members directly
square(5);
cube(10);
PI;
```

**Trade-offs:**
- ✅ Convenient for REPL and scripts
- ✅ Common in functional languages (Haskell, OCaml)
- ❌ Name collisions harder to debug
- ❌ Unclear where symbols come from

**Recommendation:** Implement, but:
1. Generate linter warnings for wildcard imports in modules (only allow in scripts)
2. Require explicit collision resolution

**Estimated Effort:** 1-2 days (builds on selective imports)

---

## 5. Module Documentation (MEDIUM PRIORITY)

### Proposed Syntax

**Triple-slash comments for docstrings:**
```flux
/// Math utilities for common operations
///
/// Provides functions for arithmetic, trigonometry,
/// and mathematical constants.
module Modules.Math {
  /// Compute the square of a number
  ///
  /// # Examples
  /// ```flux
  /// Math.square(5)  // => 25
  /// ```
  pub fun square(x) {
    x * x
  }

  /// The mathematical constant pi
  pub let PI = 3.14159
}
```

### Implementation

**5a. Add Documentation to AST**
```rust
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionStatement {
    pub doc_comment: Option<String>,  // NEW
    pub visibility: Visibility,
    pub name: String,
    pub parameters: Vec<String>,
    pub body: Block,
    pub position: Position,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleStatement {
    pub doc_comment: Option<String>,  // NEW
    pub name: Vec<String>,
    pub body: Vec<Statement>,
    pub position: Position,
}
```

**5b. Update Lexer** ([src/frontend/lexer.rs](src/frontend/lexer.rs))
```rust
// Collect doc comments (///) as tokens
fn skip_whitespace(&mut self) {
    while self.ch.is_ascii_whitespace() {
        self.read_char();
    }

    // Check for comments
    if self.ch == '/' && self.peek_char() == '/' {
        if self.peek_char_n(2) == '/' {
            // Doc comment: ///
            self.current_doc_comment = Some(self.read_doc_comment());
        } else {
            // Regular comment
            self.skip_comment();
        }
    }
}
```

**5c. Documentation Generator Command**
```bash
cargo run -- doc src/modules/ --output docs/
```

Generates HTML documentation from docstrings.

**Estimated Effort:** 3-4 days

---

## 6-9. Standard Library Modules (HIGH PRIORITY)

### Module Structure
```
stdlib/
├── Flow/
│   ├── List.flx        # Array/list utilities
│   ├── Option.flx      # Option type helpers
│   ├── Result.flx      # Result/Either helpers
│   ├── String.flx      # String utilities
│   ├── Math.flx        # Mathematical functions
│   ├── Debug.flx       # Debugging utilities
│   └── Prelude.flx     # Auto-imported basics
```

### Implementation Plan

**6. Flow.List Module**
```flux
/// List manipulation and transformation utilities
module Flow.List {
  /// Map a function over a list
  pub fun map(arr, f) {
    let result = [];
    let i = 0;
    while i < len(arr) {
      result = push(result, f(arr[i]));
      i = i + 1;
    }
    result;
  }

  /// Filter list by predicate
  pub fun filter(arr, pred) { /* ... */ }

  /// Reduce list to single value
  pub fun reduce(arr, init, f) { /* ... */ }

  /// Find first element matching predicate
  pub fun find(arr, pred) { /* ... */ }

  /// Check if all elements satisfy predicate
  pub fun all(arr, pred) { /* ... */ }

  /// Check if any element satisfies predicate
  pub fun any(arr, pred) { /* ... */ }

  /// Take first n elements
  pub fun take(arr, n) { /* ... */ }

  /// Drop first n elements
  pub fun drop(arr, n) { /* ... */ }

  /// Reverse a list
  pub fun reverse(arr) { /* ... */ }

  /// Flatten nested lists
  pub fun flatten(arr) { /* ... */ }

  /// Zip two lists together
  pub fun zip(a, b) { /* ... */ }
}
```

**7. Flow.Option Module**
```flux
/// Option type utilities for handling None/Some
module Flow.Option {
  /// Unwrap with default value
  pub fun unwrap_or(opt, default) {
    match opt {
      Some(value) -> value;
      None -> default;
    }
  }

  /// Map over Option
  pub fun map(opt, f) {
    match opt {
      Some(value) -> Some(f(value));
      None -> None;
    }
  }

  /// Chain Option operations
  pub fun and_then(opt, f) {
    match opt {
      Some(value) -> f(value);
      None -> None;
    }
  }

  /// Check if Option is Some
  pub fun is_some(opt) { /* ... */ }

  /// Check if Option is None
  pub fun is_none(opt) { /* ... */ }
}
```

**8. Flow.Result Module**
```flux
/// Result type utilities for error handling
module Flow.Result {
  /// Unwrap Result or return default
  pub fun unwrap_or(result, default) {
    match result {
      Right(value) -> value;
      Left(_) -> default;
    }
  }

  /// Map over success value
  pub fun map(result, f) {
    match result {
      Right(value) -> Right(f(value));
      Left(err) -> Left(err);
    }
  }

  /// Map over error value
  pub fun map_err(result, f) { /* ... */ }

  /// Chain Result operations
  pub fun and_then(result, f) { /* ... */ }

  /// Check if Result is Ok
  pub fun is_ok(result) { /* ... */ }

  /// Check if Result is Err
  pub fun is_err(result) { /* ... */ }
}
```

**9. Flow.String Module**
```flux
/// String manipulation utilities
module Flow.String {
  /// Split string by delimiter
  pub fun split(s, delim) { /* ... */ }

  /// Join array of strings
  pub fun join(arr, sep) { /* ... */ }

  /// Trim whitespace
  pub fun trim(s) { /* ... */ }

  /// Convert to uppercase
  pub fun upper(s) { /* ... */ }

  /// Convert to lowercase
  pub fun lower(s) { /* ... */ }

  /// Check if string starts with prefix
  pub fun starts_with(s, prefix) { /* ... */ }

  /// Check if string ends with suffix
  pub fun ends_with(s, suffix) { /* ... */ }

  /// Replace substring
  pub fun replace(s, old, new) { /* ... */ }
}
```

### Standard Library Integration

**Option A: Built-in Modules (Recommended)**
- Ship stdlib as bytecode embedded in binary
- Always available, no imports needed for core types
- Fast startup, no I/O

**Option B: Source Distribution**
- Ship stdlib as `.flx` files
- Install to `~/.flux/stdlib/`
- Compile on first use, cache bytecode

**Option C: Prelude System**
```flux
// Flow.Prelude automatically imported in every file
module Flow.Prelude {
  export { map, filter, reduce } from Flow.List
  export { unwrap_or, is_some, is_none } from Flow.Option
  export { is_ok, is_err } from Flow.Result
}
```

**Estimated Effort:** 1-2 weeks for all stdlib modules

---

## 10. Module Metadata (MEDIUM PRIORITY)

### Proposed Format

**In-module metadata:**
```flux
/// Math utilities for common operations
///
/// @version 0.1.0
/// @author Flux Team
/// @license MIT
module Modules.Math {
  // ...
}
```

**External manifest (flux.toml):**
```toml
[package]
name = "math-utils"
version = "0.1.0"
authors = ["Flux Team"]
license = "MIT"
description = "Mathematical utility functions"

[dependencies]
# Future: external dependencies
```

**Estimated Effort:** 2-3 days

---

---

## Part B: Compiler-Level Module Improvements

The following improvements focus on **how the compiler handles modules internally** rather than language-level features.

---

## 11. Parallel Module Compilation (HIGH PRIORITY)

### Current Behavior
- Modules compiled sequentially in topological order
- Single-threaded compilation
- Wastes CPU on multi-core systems

### Problem
```
Module A → Module B → Module C
Module D → Module E → Module F
```

Currently: `A → B → C → D → E → F` (sequential, ~600ms)
Could be: `(A,D) → (B,E) → (C,F)` (parallel, ~200ms)

### Proposed Solution

**Parallel compilation respecting dependencies:**
1. Build dependency graph (already done ✅)
2. Identify independent modules (same depth in topo order)
3. Compile independent modules in parallel using thread pool
4. Wait for dependencies before compiling dependents

### Implementation

**Add parallel compilation** ([src/bytecode/compiler/parallel.rs](src/bytecode/compiler/parallel.rs))
```rust
use rayon::prelude::*;
use std::sync::Arc;

pub struct ParallelCompiler {
    module_graph: Arc<ModuleGraph>,
    thread_pool: rayon::ThreadPool,
}

impl ParallelCompiler {
    pub fn compile_all(&self) -> Result<HashMap<ModuleId, Bytecode>, Vec<Diagnostic>> {
        let topo_order = self.module_graph.topo_order();
        let mut compiled = HashMap::new();
        let mut errors = Vec::new();

        // Group modules by depth (can compile same depth in parallel)
        let depth_groups = self.group_by_depth(topo_order);

        for group in depth_groups {
            // Compile all modules at this depth in parallel
            let results: Vec<_> = group.par_iter()
                .map(|module| self.compile_module(module, &compiled))
                .collect();

            for result in results {
                match result {
                    Ok((id, bytecode)) => {
                        compiled.insert(id, bytecode);
                    }
                    Err(diag) => errors.push(diag),
                }
            }

            if !errors.is_empty() {
                return Err(errors);
            }
        }

        Ok(compiled)
    }

    fn group_by_depth(&self, topo_order: &[ModuleId]) -> Vec<Vec<ModuleId>> {
        let mut depth_map: HashMap<ModuleId, usize> = HashMap::new();

        for module_id in topo_order {
            let max_dep_depth = self.module_graph.get_dependencies(module_id)
                .iter()
                .map(|dep| depth_map.get(dep).unwrap_or(&0))
                .max()
                .unwrap_or(0);

            depth_map.insert(module_id.clone(), max_dep_depth + 1);
        }

        // Group by depth
        let mut groups: HashMap<usize, Vec<ModuleId>> = HashMap::new();
        for (module_id, depth) in depth_map {
            groups.entry(depth).or_default().push(module_id);
        }

        let mut sorted_groups: Vec<_> = groups.into_iter().collect();
        sorted_groups.sort_by_key(|(depth, _)| *depth);
        sorted_groups.into_iter().map(|(_, group)| group).collect()
    }
}
```

**Command-line flag:**
```bash
flux run --parallel main.flx       # Enable parallel compilation
flux run --jobs 4 main.flx         # Use 4 threads
```

**Benefits:**
- 2-5x faster compilation for projects with >10 modules
- Better CPU utilization on multi-core systems
- Scales with number of independent modules

**Estimated Effort:** 4-5 days

---

## 12. Incremental Compilation (HIGH PRIORITY)

### Current Behavior
- Bytecode cache per module (`.fxc` files)
- Cache invalidated on source change
- All dependencies recompiled even if unchanged

### Proposed Improvement

**Dependency-aware caching:**
1. Hash module source + dependency hashes
2. Skip compilation if hash matches cached bytecode
3. Only recompile modules with changed dependencies

**Implementation:**
```rust
// In bytecode_cache
struct CacheMetadata {
    source_hash: u64,
    dependency_hashes: HashMap<ModuleId, u64>,  // NEW
    compiler_version: String,
}

fn should_recompile(
    module: &ModuleNode,
    cache: &CacheMetadata,
) -> bool {
    // Check if any dependency changed
    for import in &module.imports {
        let dep_hash = compute_module_hash(&import.target);
        if cache.dependency_hashes.get(&import.target) != Some(&dep_hash) {
            return true;  // Dependency changed, recompile
        }
    }
    false
}
```

**Benefits:**
- 5-10x faster rebuilds for large projects
- Only recompile changed modules + dependents

**Estimated Effort:** 3-4 days

---

## 13. Module Interface Files (.flxi) (MEDIUM PRIORITY)

### Current Behavior
- Import requires full module compilation
- Type information extracted from bytecode
- Slow for large dependency chains

### Problem
```
App imports Lib1 imports Lib2 imports Lib3
└─> Must compile Lib3, Lib2, Lib1 before App
```

### Proposed Solution

**Generate interface files with type/export information:**
- Compile module → generate `.flxi` file with public API
- Dependent modules only read `.flxi` (don't recompile)
- Similar to C header files, Rust `.rlib`, OCaml `.cmi`

### Interface File Format

**Example: Math.flxi**
```rust
// Binary format for fast loading
ModuleInterface {
    name: "Modules.Math",
    version: "0.1.0",
    exports: [
        Export {
            name: "square",
            kind: Function {
                params: ["x"],
                arity: 1,
            },
        },
        Export {
            name: "PI",
            kind: Constant {
                value: Float(3.14159),
            },
        },
    ],
}
```

### Implementation

**Generate interface during compilation** ([src/bytecode/interface.rs](src/bytecode/interface.rs))
```rust
pub struct ModuleInterface {
    pub name: String,
    pub exports: Vec<ExportInfo>,
}

pub enum ExportInfo {
    Function { name: String, arity: usize },
    Constant { name: String, value: Object },
}

impl ModuleInterface {
    pub fn from_module(module: &ModuleNode, bytecode: &Bytecode) -> Self {
        let mut exports = Vec::new();

        for stmt in &module.program.statements {
            match stmt {
                Statement::Function { visibility: Visibility::Public, name, parameters, .. } => {
                    exports.push(ExportInfo::Function {
                        name: name.clone(),
                        arity: parameters.len(),
                    });
                }
                Statement::Let { visibility: Visibility::Public, name, .. } => {
                    // Extract constant value from bytecode if possible
                    if let Some(value) = bytecode.get_constant(name) {
                        exports.push(ExportInfo::Constant {
                            name: name.clone(),
                            value: value.clone(),
                        });
                    }
                }
                _ => {}
            }
        }

        ModuleInterface {
            name: module.id.as_str().to_string(),
            exports,
        }
    }

    pub fn save_to_file(&self, path: &Path) -> Result<(), std::io::Error> {
        let bytes = bincode::serialize(self).unwrap();
        std::fs::write(path, bytes)
    }

    pub fn load_from_file(path: &Path) -> Result<Self, std::io::Error> {
        let bytes = std::fs::read(path)?;
        Ok(bincode::deserialize(&bytes).unwrap())
    }
}
```

**Use interfaces during import resolution:**
```rust
// In compiler
fn resolve_import(&mut self, module_path: &[String]) -> Result<ModuleInterface> {
    let interface_path = self.find_interface_file(module_path)?;

    if interface_path.exists() {
        // Fast path: load interface without recompiling
        ModuleInterface::load_from_file(&interface_path)
    } else {
        // Slow path: compile module and generate interface
        self.compile_dependency(module_path)
    }
}
```

**Benefits:**
- 10-20x faster import resolution (no recompilation)
- Enables better tooling (LSP can read interfaces quickly)
- Smaller files than full bytecode

**Estimated Effort:** 5-6 days

---

## 14. Cross-Module Optimization (LOW PRIORITY)

### Current Behavior
- Each module optimized independently
- No cross-module inlining
- No dead code elimination across modules

### Proposed Optimizations

**14a. Function Inlining Across Modules**
```flux
// Math.flx
module Math {
  pub fun square(x) { x * x }  // Small, should inline
}

// App.flx
import Math { square }

fun compute() {
  square(5);  // Inline to: 5 * 5
}
```

**14b. Dead Code Elimination**
```flux
// Utils.flx
module Utils {
  pub fun used() { 1 }
  pub fun unused() { 2 }  // Never imported
}

// App.flx
import Utils { used }
// Don't include 'unused' in final bytecode
```

**14c. Constant Propagation**
```flux
// Config.flx
module Config {
  pub let DEBUG = false;
}

// App.flx
import Config { DEBUG }

if DEBUG {  // Eliminate entire branch at compile time
  print("debug mode");
}
```

**Implementation:**
```rust
// In bytecode/optimizer.rs
pub struct CrossModuleOptimizer {
    module_graph: ModuleGraph,
    inlining_threshold: usize,  // Inline functions < N instructions
}

impl CrossModuleOptimizer {
    pub fn inline_small_functions(&mut self, bytecode: &mut Bytecode) {
        for call in bytecode.find_cross_module_calls() {
            if let Some(target) = self.resolve_function(&call.module, &call.function) {
                if target.instruction_count() < self.inlining_threshold {
                    bytecode.inline_call(call.position, &target.instructions);
                }
            }
        }
    }

    pub fn eliminate_dead_exports(&mut self) {
        // Track which exports are actually imported
        let used_exports = self.find_used_exports();

        for module in self.module_graph.modules_mut() {
            module.remove_unused_exports(&used_exports);
        }
    }
}
```

**Benefits:**
- 5-15% smaller bytecode
- 5-10% faster execution (inlining)
- Better tree-shaking for libraries

**Estimated Effort:** 1-2 weeks

---

## 15. Module Precompilation & AOT (LOW PRIORITY)

### Current Behavior
- Standard library compiled on every run
- No ahead-of-time compilation
- Slow startup for large projects

### Proposed Solution

**Ahead-of-Time (AOT) compilation:**
```bash
# Precompile standard library
flux compile --aot stdlib/ --output ~/.flux/precompiled/

# Use precompiled stdlib
flux run --use-precompiled main.flx  # 10x faster startup
```

**Implementation:**
```rust
// Embed precompiled bytecode in binary
const STDLIB_BYTECODE: &[u8] = include_bytes!("../stdlib.fxc");

pub fn load_stdlib() -> HashMap<ModuleId, Bytecode> {
    bincode::deserialize(STDLIB_BYTECODE).unwrap()
}
```

**Benefits:**
- Near-instant startup (no stdlib compilation)
- Better user experience for CLI tools
- Easier distribution (bundle precompiled stdlib)

**Estimated Effort:** 3-4 days

---

## 16. Lazy Module Loading (LOW PRIORITY)

### Current Behavior
- All imported modules loaded at startup
- Wastes memory for unused code paths

### Proposed Solution

**Load modules on-demand:**
```flux
// Only load Heavy module if condition is true
import Heavy { expensive_function }

if user_requested {
  expensive_function();  // Load Heavy.flx here
}
```

**Implementation:**
```rust
// In VM
pub enum LoadedModule {
    Loaded(Bytecode),
    Lazy { path: PathBuf, interface: ModuleInterface },
}

impl VM {
    fn call_module_function(&mut self, module: &str, function: &str) {
        let module = self.modules.get_mut(module).unwrap();

        match module {
            LoadedModule::Loaded(bytecode) => {
                // Already loaded, execute
                self.execute_function(bytecode, function)
            }
            LoadedModule::Lazy { path, .. } => {
                // Load module now
                let bytecode = self.load_module(path);
                *module = LoadedModule::Loaded(bytecode);
                self.execute_function(&bytecode, function)
            }
        }
    }
}
```

**Benefits:**
- Lower memory usage (only load what's used)
- Faster startup (defer compilation)
- Better for large applications

**Estimated Effort:** 4-5 days

---

## 17. Module Compilation Pipeline Visualization (LOW PRIORITY)

### Debugging Aid

**Show compilation progress:**
```bash
flux build --verbose

Building module graph...
  ✓ Found 15 modules
  ✓ Detected 0 cycles
  ✓ Topological order: [A, B, C, D, ...]

Compiling modules (parallel, 4 threads):
  [1/15] Compiling Flow.List... (125ms)
  [2/15] Compiling Flow.Option... (98ms)
  [3/15] Compiling Flow.Result... (102ms)
  [4/15] Compiling Flow.String... (156ms)
  ...
  [15/15] Compiling Main... (45ms)

Total: 1.2s (800ms compilation, 400ms I/O)

Cache statistics:
  ✓ 12/15 modules cached (80%)
  ✓ 3/15 modules recompiled
  ✓ Saved 2.1s
```

**Estimated Effort:** 2-3 days

---

## 18. Module Symbol Table Optimization (MEDIUM PRIORITY)

### Current Behavior
- Symbol tables rebuilt for each compilation
- No sharing between modules
- Duplicate symbol storage

### Proposed Optimization

**Shared symbol interner across modules:**
```rust
pub struct ModuleCompiler<'ctx> {
    interner: &'ctx SymbolInterner,  // Shared across all modules
    module_symbols: SymbolTable,      // Module-specific
}

// Symbols interned once, referenced by ID
let square_id = interner.intern("square");  // Only once
let cube_id = interner.intern("cube");

// Reuse in multiple modules
module_a.symbols.insert(square_id, ...);
module_b.symbols.insert(square_id, ...);  // Same ID
```

**Benefits:**
- 50% less memory (deduplication)
- Faster symbol comparison (integer equality)
- Foundation for future type system

**Note:** This builds on [Proposal 005: Symbol Interning](005_symbol_interning.md)

**Estimated Effort:** Covered in Proposal 005 (defer to Phase 3)

---

## Compiler Module Improvements Summary

| Feature | Priority | Effort | Benefit |
|---------|----------|--------|---------|
| **Parallel Compilation** | HIGH | 4-5 days | 2-5x faster builds |
| **Incremental Compilation** | HIGH | 3-4 days | 5-10x faster rebuilds |
| **Module Interface Files** | MEDIUM | 5-6 days | 10-20x faster imports |
| **Cross-Module Optimization** | LOW | 1-2 weeks | 5-15% smaller/faster code |
| **AOT Precompilation** | LOW | 3-4 days | 10x faster startup |
| **Lazy Module Loading** | LOW | 4-5 days | Lower memory usage |
| **Pipeline Visualization** | LOW | 2-3 days | Better DX |
| **Symbol Table Optimization** | MEDIUM | Phase 3 | 50% less memory |

---

## 19. Documentation Generator (LOW PRIORITY)

### Command
```bash
flux doc src/ --output docs/html/
```

### Features
- Extract docstrings from modules
- Generate HTML with syntax highlighting
- Cross-reference links between modules
- Search functionality

**Estimated Effort:** 1 week

---

## 13-14. Package System (LOW PRIORITY)

### Manifest Format (`flux.toml`)
```toml
[package]
name = "my-app"
version = "0.1.0"
authors = ["Your Name"]
edition = "2026"

[dependencies]
# Local path dependencies
math-utils = { path = "../math-utils" }

# Future: registry dependencies
# http-client = "1.0"
```

### Package Resolution
```bash
flux build              # Resolve dependencies, compile
flux run                # Run main module
flux test               # Run tests
flux publish            # Publish to registry (future)
```

**Estimated Effort:** 1-2 weeks

---

## Implementation Roadmap

### Phased Approach

We recommend implementing in **three tracks** that can run in parallel:
- **Track A:** Language Features (selective imports, re-exports, visibility)
- **Track B:** Standard Library (Flow.* modules)
- **Track C:** Compiler Optimizations (parallel compilation, caching)

---

### Track A: Language Features (3 weeks)

#### Week 1: Selective Imports
**Deliverable:** `import Foo { bar, baz }` syntax

- Day 1-2: Update AST and parser for `ImportSpecifier`
- Day 3-4: Compiler support for selective binding
- Day 5: Wildcard imports (`import Foo.*`)
- Day 6-7: Tests and error handling

#### Week 2: Re-exports & Visibility
**Deliverable:** `export { ... }` and `pub` keyword

- Day 8-9: Re-export syntax in parser
- Day 10-11: Module export tracking
- Day 12: `pub` keyword for functions/constants
- Day 13-14: Tests and documentation

#### Week 3: Module Documentation
**Deliverable:** Docstring support and basic docs

- Day 15-16: Lexer support for `///` doc comments
- Day 17-18: AST integration for docstrings
- Day 19-21: Basic documentation generator

---

### Track B: Standard Library (2 weeks)

#### Week 4: Core Modules
**Deliverable:** Flow.List, Flow.Option, Flow.Result

- Day 22-23: Flow.List (map, filter, reduce, find, etc.)
- Day 24: Flow.Option (unwrap_or, map, and_then)
- Day 25: Flow.Result (map, map_err, is_ok)
- Day 26-28: Comprehensive tests for all stdlib functions

#### Week 5: Utility Modules
**Deliverable:** Flow.String, Flow.Math, Flow.Debug

- Day 29-30: Flow.String (split, join, trim, etc.)
- Day 31: Flow.Math (abs, min, max, etc.)
- Day 32: Flow.Debug (inspect, trace, assert)
- Day 33-35: Integration testing, documentation, examples

---

### Track C: Compiler Optimizations (3 weeks)

#### Week 6: Parallel Compilation
**Deliverable:** Multi-threaded module compilation

- Day 36-37: Dependency depth grouping algorithm
- Day 38-39: Thread pool integration (rayon)
- Day 40: Parallel compilation implementation
- Day 41-42: Performance benchmarks and tuning

#### Week 7: Incremental Compilation
**Deliverable:** Smart cache invalidation

- Day 43-44: Dependency hash tracking
- Day 45-46: Cache validation with dep hashes
- Day 47-48: Integration testing
- Day 49: Performance validation (5-10x faster rebuilds)

#### Week 8: Module Interface Files
**Deliverable:** `.flxi` files for fast imports

- Day 50-51: Interface file format design
- Day 52-53: Interface generation during compilation
- Day 54-55: Interface loading for imports
- Day 56: Testing and performance validation

---

### Optional Extensions (Week 9+)

**If time permits:**
- Cross-module optimization (inlining, DCE)
- AOT precompilation for stdlib
- Lazy module loading
- Compilation pipeline visualization
- Package manifest format (flux.toml)

---

## Success Metrics

### Code Quality
- ✅ All existing tests pass (100% backward compatibility)
- ✅ New module system features have >90% test coverage
- ✅ Standard library has comprehensive test suite (>200 unit tests)
- ✅ No performance regressions in existing code

### Developer Experience (Language Features)
- ✅ Can write `import Math { square }` for selective imports
- ✅ Can write `export { foo } from Bar` for re-exports
- ✅ Can use `pub fun` for explicit visibility
- ✅ Standard library available: Flow.List, Flow.Option, Flow.Result, Flow.String
- ✅ Documentation generated from `///` docstrings
- ✅ Clear error messages for import/export issues

### Performance (Compiler Improvements)
- ✅ **2-5x faster compilation** with parallel compilation (>10 modules)
- ✅ **5-10x faster rebuilds** with incremental compilation
- ✅ **10-20x faster imports** with interface files
- ✅ **50% less memory** with shared symbol tables (Phase 3)
- ✅ Compile time < 100ms for small projects

### Ecosystem Readiness
- ✅ Official standard library shipped with Flux
- ✅ Module metadata system in place
- ✅ Package manifest format defined (flux.toml)
- ✅ Local package dependencies work
- ✅ Foundation for future package registry

---

## Complete Feature Comparison

### Before Phase 2 (Current)
```flux
// Import entire module
import Math

// Use qualified access
Math.square(5);
Math.cube(10);

// No standard library
fun map(arr, f) {
  // Implement yourself
}

// No module documentation
// No incremental builds
// Sequential compilation
```

### After Phase 2 (Proposed)
```flux
// Selective imports
import Math { square, cube }
import Flow.List { map, filter, reduce }

// Direct usage
square(5);
map([1, 2, 3], fun(x) { x * 2 });

/// Math utilities module
module MyMath {
  /// Square a number
  pub fun square(x) { x * x }
}

// Parallel compilation: 3x faster
// Incremental builds: 8x faster rebuilds
// Interface files: 15x faster imports
```

---

## Risks and Mitigation

### Risk 1: Breaking Changes
**Likelihood:** Medium
**Impact:** High
**Mitigation:**
- Selective imports are additive (backward compatible)
- `pub` keyword defaults to public initially
- Provide migration guide for v0.2.0 breaking changes

### Risk 2: Standard Library Design Lock-in
**Likelihood:** Medium
**Impact:** Medium
**Mitigation:**
- Start with minimal stdlib (List, Option, Result, String)
- Mark as experimental in v0.1.x
- Gather community feedback before stabilizing API

### Risk 3: Implementation Complexity
**Likelihood:** Low
**Impact:** Medium
**Mitigation:**
- Build incrementally (selective imports → re-exports → stdlib)
- Comprehensive test suite for each feature
- Code review for module system changes

---

## Future Considerations (Phase 3+)

### Post-Phase 2 Opportunities
1. **Package Registry** - Central repository for third-party packages
2. **Semantic Versioning** - Dependency version resolution
3. **Module Macros** - Code generation at compile time
4. **FFI Modules** - Call external libraries (C, Rust)
5. **Hot Module Reloading** - Update modules without restart

---

## References

- [Module Graph Documentation](../architecture/module_graph.md)
- [Language Design](../language/language_design.md)
- [Stdlib Proposal](003_stdlib_proposal.md)
- [Phase 1 Module Split](006_phase1_module_split_plan.md)

---

---

## Summary: What Phase 2 Delivers

### Part A: Language Features (User-Facing)
1. ✅ **Selective imports** - `import Foo { bar, baz }`
2. ✅ **Re-exports** - `export { bar } from Foo`
3. ✅ **Explicit visibility** - `pub fun`, `pub let`
4. ✅ **Wildcard imports** - `import Foo.*`
5. ✅ **Module documentation** - `///` docstrings
6. ✅ **Standard library** - Flow.List, Flow.Option, Flow.Result, Flow.String
7. ✅ **Module metadata** - Version, author, description

### Part B: Compiler Improvements (Performance)
8. ✅ **Parallel compilation** - 2-5x faster for large projects
9. ✅ **Incremental compilation** - 5-10x faster rebuilds
10. ✅ **Module interface files** - 10-20x faster imports
11. ✅ **Cross-module optimization** - 5-15% smaller/faster code (optional)
12. ✅ **AOT precompilation** - Near-instant startup (optional)

### Impact
- **Developer productivity:** 5-10x faster iteration with incremental builds
- **Code organization:** Better modularity with selective imports and re-exports
- **Ecosystem growth:** Standard library enables building real applications
- **Language maturity:** Documentation and visibility make Flux production-ready

---

## Approval Checklist

### Language Features
- [ ] Selective imports syntax approved
- [ ] Re-export semantics agreed upon
- [ ] Visibility model finalized (`pub` vs `_` prefix)
- [ ] Standard library API reviewed
- [ ] Module documentation format approved

### Compiler Improvements
- [ ] Parallel compilation strategy approved
- [ ] Incremental compilation cache format reviewed
- [ ] Interface file format agreed upon
- [ ] Performance benchmarks baseline established

### Implementation
- [ ] Three-track roadmap (A/B/C) approved
- [ ] Timeline agreed upon (8 weeks core, 2+ weeks optional)
- [ ] Testing strategy approved
- [ ] Migration guide planned

### Ready to Proceed
- [ ] All stakeholders reviewed
- [ ] Priority features identified
- [ ] Risk assessment complete
- [ ] Ready to implement

---

## Next Steps

### Immediate (Week 1)
1. Review and approve proposal
2. Set up performance benchmarking infrastructure
3. Begin Track A: Selective imports implementation

### Short-term (Weeks 2-8)
1. Implement core language features (Track A)
2. Build standard library modules (Track B)
3. Add compiler optimizations (Track C)

### Long-term (Phase 3+)
1. Package registry for third-party modules
2. Semantic versioning and dependency resolution
3. Module macros and code generation
4. FFI for external libraries

---

**Recommendation:** Start with Track A (language features) as it has the highest user-facing impact, then parallelize Track B (stdlib) and Track C (compiler optimizations).
