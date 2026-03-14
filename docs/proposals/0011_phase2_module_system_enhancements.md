- Feature Name: Phase 2 - Module System Enhancements
- Start Date: 2026-02-04
- Status: Partially Implemented
- Proposal PR:
- Flux Issue:

# Proposal 0011: Phase 2 - Module System Enhancements

## Summary
[summary]: #summary

This proposal outlines comprehensive module system improvements for Phase 2. Building on Phase 1's code organization success, Phase 2 focuses on **module functionality** - adding selective imports, re-exports, explicit visibility, package management foundations, and standard library infrastructure.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

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

### Proposed Syntax

**Option A: Export Declaration (Recommended)**
```flux
module Utils {
  import Math { square, cube }

  export { square, cube }  // Re-export imported symbols

  pub fn double(x) { x * 2 }
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

### Proposed Syntax

**Option A: Rust-style `pub` Keyword (Recommended)**
```flux
module Math {
  pub fn square(x) { x * x }     // Explicit public
  fn helper(x) { x + 1 }         // Private by default

  pub let PI = 3.14159            // Public constant
  let EPSILON = 0.0001            // Private constant
}
```

**Option B: Keep `_` prefix, add `pub` for constants**
```flux
module Math {
  fn square(x) { x * x }         // Public (backward compatible)
  fn _helper(x) { x + 1 }        // Private

  pub let PI = 3.14159            // Public constant (new)
  let _EPSILON = 0.0001           // Private constant
}
```

**Recommendation:** Option A for consistency, but provide migration guide.

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
  pub fn square(x) {
    x * x
  }

  /// The mathematical constant pi
  pub let PI = 3.14159
}
```

### Risk 2: Standard Library Design Lock-in

**Likelihood:** Medium
**Impact:** Medium
**Mitigation:**
- Start with minimal stdlib (List, Option, Result, String)
- Mark as experimental in v0.1.x
- Gather community feedback before stabilizing API

### Proposed Syntax

### Proposed Syntax

### Proposed Syntax

### Proposed Syntax

**Estimated Effort:** 1-2 days (builds on selective imports)

### Proposed Syntax

### Risk 2: Standard Library Design Lock-in

### Risk 2: Standard Library Design Lock-in

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **Impact:** **For Users:** - ❌ Can't write `import Math { square, cube }` - pollutes namespace - ❌ Can't build modular libraries with re-exports - ❌ No standard library for co...
- **Impact:** **For Users:** - ❌ Can't write `import Math { square, cube }` - pollutes namespace - ❌ Can't build modular libraries with re-exports - ❌ No standard library for common tasks (Li...
- **In Scope (Phase 2):** **Priority 1 (HIGH) - Core Language Features:** 1. ✅ Selective imports (`import Foo { bar, baz }`) 2. ✅ Re-exports (`export { bar } from Foo`) 3. ✅ Explicit visibility (`pub fn`...
- **Out of Scope:** - ❌ Remote package registry (defer to Phase 4) - ❌ Semantic versioning / dependency resolution - ❌ Module hot reloading - ❌ FFI / external modules - ❌ Module macros / code gener...
- **Current Behavior:** // Must use qualified access for everything Math.square(5); Math.cube(10); ```
- **Implementation:** **1a. Update AST** ([src/syntax/statement.rs](src/syntax/statement.rs)) ```rust #[derive(Debug, Clone, PartialEq)] pub enum ImportSpecifier { /// Import entire module: `import M...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### Risk 2: Standard Library Design Lock-in

**Likelihood:** Medium
**Impact:** Medium
**Mitigation:**
- Start with minimal stdlib (List, Option, Result, String)
- Mark as experimental in v0.1.x
- Gather community feedback before stabilizing API

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

### Risk 1: Breaking Changes

**Likelihood:** Medium
**Impact:** High
**Mitigation:**
- Selective imports are additive (backward compatible)
- `pub` keyword defaults to public initially
- Provide migration guide for v0.2.0 breaking changes

### Risk 2: Standard Library Design Lock-in

### Risk 3: Implementation Complexity

**Likelihood:** Low
**Impact:** Medium
**Mitigation:**
- Build incrementally (selective imports → re-exports → stdlib)
- Comprehensive test suite for each feature
- Code review for module system changes

### Current Module System Limitations

### Risk 1: Breaking Changes

### Risk 2: Standard Library Design Lock-in

### Risk 3: Implementation Complexity

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### References

- [Module Graph Documentation](../architecture/module_graph.md)
- [Language Design](../language/language_design.md)
- [Stdlib Proposal](0003_stdlib_proposal.md)
- [Phase 1 Module Split](implemented/0006_phase1_module_split_plan.md)

### References

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

### Future: external dependencies

```

**Estimated Effort:** 2-3 days

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
