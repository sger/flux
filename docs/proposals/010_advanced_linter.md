# Proposal 010: Advanced Linter (Post v0.1.0)

**Status:** Draft
**Created:** 2026-02-02
**Target Version:** v0.2.0+
**Prerequisites:** v0.1.0 release, Macro System (Proposal 009)

---

## Overview

This proposal outlines a comprehensive linter enhancement plan for Flux, introducing advanced code quality checks, configurability, and integration with development workflows.

**Timeline:** 6-8 weeks (post v0.1.0)
**Complexity:** Medium-High

---

## Current State (v0.0.3)

### Existing Warnings

| Code | Warning | Status |
|------|---------|--------|
| W001 | Unused variable | ✅ Working |
| W002 | Unused parameter | ✅ Working |
| W003 | Unused import | ✅ Working |
| W004 | Shadowed name | ✅ Working |
| W005 | Function name style | ✅ Working |
| W006 | Import name style | ✅ Working |
| W007 | Unused function | ✅ Working (v0.0.2) |
| W008 | Dead code after return | ⏳ Planned (v0.0.3) |
| W009 | Function too long | ⏳ Planned (v0.0.3) |
| W010 | Too many parameters | ⏳ Planned (v0.0.3) |

### Limitations

- ❌ No configuration support
- ❌ No unused function result detection
- ❌ No complexity metrics
- ❌ No magic number detection
- ❌ No documentation coverage
- ❌ No custom rule support

---

## Goals for v0.2.0+

### Primary Objectives

1. **Configurability** - Let users customize lint rules
2. **Advanced Checks** - Add sophisticated code quality analysis
3. **IDE Integration** - LSP support for real-time linting
4. **Custom Rules** - Enable user-defined lint rules via macros
5. **Performance** - Lint large codebases efficiently

---

## New Warning Categories

### 1. Result Usage (W100-W109)

**W101: Unused Function Result**
```flux
calculate_total(items);  // ← W101: Result not used
// Should be: let total = calculate_total(items);
```

**W102: Unused Error Result**
```flux
match try_open_file(path) {
    Left(err) -> ();  // ← W102: Error ignored without handling
    Right(file) -> process(file);
}
```

**Implementation:**
- Track function calls in expression statements
- Check if return value is `None` type
- Flag non-unit returns that are discarded

---

### 2. Code Smell Detection (W110-W119)

**W111: Magic Number**
```flux
fun calculate_tax(amount) {
    amount * 0.19;  // ← W111: Magic number 0.19
}

// Better:
let TAX_RATE = 0.19;
fun calculate_tax(amount) {
    amount * TAX_RATE;
}
```

**W112: Boolean Literal in Condition**
```flux
if x == true {  // ← W112: Redundant comparison
    // ...
}

// Better:
if x {
    // ...
}
```

**W113: Redundant Pattern**
```flux
match value {
    Some(x) -> x;
    _ -> None;  // ← W113: Wildcard after Some is redundant (only None left)
}
```

---

### 3. Complexity Metrics (W120-W129)

**W121: Cyclomatic Complexity**
```flux
fun process(x) {  // ← W121: Cyclomatic complexity: 15 (max: 10)
    if x > 0 {
        if x < 10 {
            // ... many branches
        }
    }
}
```

**W122: Cognitive Complexity**
```flux
fun nested_loops() {  // ← W122: Cognitive complexity: 25 (max: 15)
    for item in items {
        if item.valid {
            for child in item.children {
                // ... deeply nested
            }
        }
    }
}
```

**Implementation:**
- Count decision points (if/match/for)
- Track nesting levels
- Calculate McCabe complexity

---

### 4. Style & Consistency (W130-W139)

**W131: Inconsistent Naming**
```flux
let user_name = "Alice";
let userName = "Bob";   // ← W131: Inconsistent naming style
```

**W132: Long Line**
```flux
let message = "This is a very long string that exceeds the maximum line length of 100 characters and should be split";  // ← W132: Line too long (120 > 100)
```

**W133: Missing Documentation**
```flux
// ← W133: Public function missing documentation
fun important_api(x, y) {
    // ...
}

// Better:
// Calculates the sum of x and y
fun important_api(x, y) {
    x + y
}
```

---

### 5. Performance (W140-W149)

**W141: Inefficient Loop**
```flux
for i in range(0, arr.length()) {  // ← W141: Use array iteration
    print(arr[i]);
}

// Better:
for item in arr {
    print(item);
}
```

**W142: Unnecessary Clone**
```flux
let x = expensive_value;
let y = x;  // If x not used after, don't need explicit semantics check
```

---

## Configuration System

### .flux-lint.toml

```toml
# Flux Linter Configuration

[rules]
unused-variable = "warn"      # error|warn|allow
unused-function = "allow"     # Disable if desired
dead-code = "warn"
magic-numbers = "warn"
function-length = { level = "warn", max = 50 }
parameter-count = { level = "warn", max = 5 }
cyclomatic-complexity = { level = "warn", max = 10 }
cognitive-complexity = { level = "warn", max = 15 }
line-length = { level = "warn", max = 100 }

[ignore]
# Disable specific warnings for patterns
unused-function = ["test_*", "_*"]  # Ignore test helpers
magic-numbers = ["tests/**/*.flx"]  # Allow magic numbers in tests

[style]
naming-convention = "snake_case"    # snake_case|camelCase|PascalCase
indent = "spaces"                   # spaces|tabs
indent-size = 4
```

### Per-File Overrides

```flux
// @flux-lint-disable unused-function, magic-numbers
fun legacy_code() {
    let x = 42;  // Magic number OK in this file
    // ...
}

// @flux-lint-enable unused-function
```

### Inline Directives

```flux
fun calculate() {
    // @flux-lint-disable-next-line magic-numbers
    let tax_rate = 0.19;

    // @flux-lint-disable magic-numbers
    let vat = 0.07;
    let service = 0.15;
    // @flux-lint-enable magic-numbers
}
```

---

## Custom Lint Rules (via Macros)

### Rule Definition

```flux
// In .flux-lint/rules/no_print_in_prod.flx
macro lint_rule no_print_in_prod(node) {
    match node {
        Expression::Call { function: "print", ... } if !cfg!(test) {
            lint_error("W200", "Avoid print() in production code", node.span)
        }
        _ -> Ok(())
    }
}
```

### Usage

```toml
# .flux-lint.toml
[custom-rules]
no-print-in-prod = { level = "warn", path = ".flux-lint/rules/no_print_in_prod.flx" }
```

---

## IDE Integration

### Language Server Protocol (LSP)

```json
{
  "textDocument/publishDiagnostics": {
    "uri": "file:///path/to/file.flx",
    "diagnostics": [
      {
        "range": { "start": { "line": 5, "character": 10 }, "end": { "line": 5, "character": 15 } },
        "severity": 2,  // Warning
        "code": "W007",
        "source": "flux-lint",
        "message": "`unused_func` is never called."
      }
    ]
  }
}
```

### Features

- ✅ Real-time diagnostics as you type
- ✅ Quick fixes for common issues
- ✅ Code actions (e.g., "Add underscore prefix to ignore")
- ✅ Hover info showing lint rule details

---

## Implementation Plan

### Phase 1: Configuration (2 weeks)

1. **Week 1:**
   - Implement .flux-lint.toml parser
   - Add configuration struct and defaults
   - Support per-rule enable/disable

2. **Week 2:**
   - Add per-file overrides
   - Implement inline directives
   - Add ignore patterns

**Deliverable:** Configurable linter with TOML support

---

### Phase 2: Advanced Checks (3 weeks)

1. **Week 3:**
   - W101-W102: Result usage tracking
   - W111-W113: Code smell detection

2. **Week 4:**
   - W121-W122: Complexity metrics
   - W131-W133: Style & consistency

3. **Week 5:**
   - W141-W142: Performance checks
   - Testing and refinement

**Deliverable:** 15+ new warning types

---

### Phase 3: Custom Rules & IDE (3 weeks)

1. **Week 6:**
   - Design macro-based custom rule API
   - Implement rule loading system
   - Add built-in rule examples

2. **Week 7:**
   - LSP integration for diagnostics
   - Real-time linting support
   - Quick fix infrastructure

3. **Week 8:**
   - Documentation and examples
   - Performance optimization
   - Integration testing

**Deliverable:** Custom rules + LSP integration

---

## Comparison with Other Linters

| Feature | Flux (Proposed) | clippy (Rust) | eslint (JS) | rubocop (Ruby) |
|---------|-----------------|---------------|-------------|----------------|
| Basic checks | ✅ | ✅ | ✅ | ✅ |
| Configuration | ✅ | ✅ | ✅ | ✅ |
| Custom rules | ✅ (Macros) | ❌ (Plugins) | ✅ (Plugins) | ✅ (Cops) |
| Complexity | ✅ | ✅ | ✅ | ✅ |
| Performance | ✅ | ✅ | ⚠️ | ⚠️ |
| IDE integration | ✅ | ✅ | ✅ | ✅ |
| Auto-fix | ⏳ Future | ✅ | ✅ | ✅ |

---

## Out of Scope (Future)

**Post v0.2.0:**
- Auto-fix suggestions
- Machine learning-based suggestions
- Team/org-wide lint profiles
- Git pre-commit hook integration
- CI/CD integration helpers
- Code coverage integration
- Security vulnerability detection

---

## Success Metrics

### v0.2.0

- [ ] 25+ lint rules implemented
- [ ] Configuration system working
- [ ] LSP integration functional
- [ ] Custom rule support via macros
- [ ] Performance: lint 10K LOC < 1 second
- [ ] Documentation complete

### Quality

- [ ] All rules have tests
- [ ] False positive rate < 5%
- [ ] No regressions from v0.0.3
- [ ] Clear error messages
- [ ] Actionable suggestions

---

## Migration Path

### From v0.0.3 to v0.2.0

**Backward Compatible:**
- All existing warnings (W001-W010) work unchanged
- Default configuration matches v0.0.3 behavior
- Opt-in for new warnings

**Breaking Changes:**
None - all new features are additive

---

## References

- **Rust clippy:** https://github.com/rust-lang/rust-clippy
- **ESLint:** https://eslint.org/
- **RuboCop:** https://rubocop.org/
- **SonarQube:** https://www.sonarqube.org/

---

## Related Proposals

- [009_macro_system.md](009_macro_system.md) - Enables custom lint rules
- [002_error_code_registry.md](002_error_code_registry.md) - Unified error format

---

## Summary

This proposal transforms Flux's linter from a basic unused-code detector into a comprehensive code quality tool with:

1. **25+ lint rules** covering correctness, style, complexity, and performance
2. **Full configurability** via .flux-lint.toml and inline directives
3. **Custom rules** powered by the macro system
4. **IDE integration** through LSP for real-time feedback
5. **Zero breaking changes** - all additive and opt-in

**Timeline:** 6-8 weeks post v0.1.0
**Risk:** Medium - LSP integration is complex, but macros provide flexibility
**Value:** High - significantly improves developer experience and code quality
