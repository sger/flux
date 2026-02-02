# Proposal 006: Phase 1 - Module Split Plan

**Status:** Planning
**Priority:** High (Maintainability)
**Created:** 2026-02-01

## Overview

This proposal outlines the module split strategy for Phase 1 of the Flux compiler architecture improvements. The goal is to improve code maintainability by breaking down large files (>500 lines) into focused, single-responsibility modules.

## Problem Statement

Three critical files exceed 800 lines and violate the Single Responsibility Principle:
- `compiler.rs` (1,671 lines) - handles expression/statement compilation, symbols, errors
- `parser.rs` (1,144 lines) - handles expression/statement parsing, utilities
- `vm.rs` (824 lines) - handles instruction dispatch, operations, tracing

This makes the codebase:
- Harder to navigate and understand
- Difficult to test in isolation
- Prone to merge conflicts
- Challenging for new contributors

## Scope

### In Scope
- Refactor 3 critical files into focused modules
- Maintain 100% backward compatibility
- Preserve all existing tests
- Document new module structure

### Out of Scope
- API changes or new features
- Performance optimizations
- Symbol interning (Phase 3)

## Detailed Plan

### 1. Compiler.rs Split (HIGH PRIORITY)

**Current:** 1,671 lines, single `impl Compiler` block
**Target:** 4 focused modules + orchestrator

#### Module Breakdown

**1a. `bytecode/compiler/expression.rs`** (280 lines)
- `compile_expression()` - Main dispatcher
- `compile_if_expression()`
- `compile_match_expression()`
- `compile_function_literal()`
- `compile_interpolated_string()`
- `compile_pattern_check()`
- `compile_pattern_bind()`
- All expression-specific logic

**1b. `bytecode/compiler/statement.rs`** (170 lines)
- `compile_statement()` - Main dispatcher
- `compile_function_statement()`
- `compile_module_statement()`
- `compile_import_statement()`
- `compile_block()`
- All statement-specific logic

**1c. `bytecode/compiler/builder.rs`** (120 lines)
- `emit()` - Instruction emission
- `add_instruction()`
- `load_symbol()`
- `add_constant()`
- `change_operand()`
- `replace_instruction()`
- `is_last_instruction()`
- `remove_last_pop()`
- `set_last_instruction()`
- Low-level bytecode generation

**1d. `bytecode/compiler/errors.rs`** (40 lines)
- `make_immutability_error()`
- `make_redeclaration_error()`
- `make_undefined_variable_error()`
- `make_import_collision_error()`
- `make_outer_assignment_error()`
- `check_private_member()`
- Diagnostic creation

**Orchestrator:** `compiler.rs` (refactored to ~200 lines)
- Compiler struct
- Public API: `new()`, `compile()`, `bytecode()`
- Module delegation
- State management

#### File Structure
```
src/bytecode/
  ├── compiler/
  │   ├── mod.rs       # Public exports, Compiler struct
  │   ├── expression.rs # Expression compilation
  │   ├── statement.rs  # Statement compilation
  │   ├── builder.rs    # Instruction emission
  │   └── errors.rs     # Error generation
  └── compiler.rs      # DEPRECATED - re-exports for compatibility
```

#### Migration Strategy
1. Create `bytecode/compiler/` directory
2. Extract methods into new modules (keep as pub(super) methods initially)
3. Update `compiler.rs` to delegate to modules
4. Run full test suite after each module
5. Document new structure

#### Estimated Effort
- 2-3 days
- Risk: Medium (touches core compilation logic)

---

### 2. Parser.rs Split (HIGH PRIORITY)

**Current:** 1,144 lines, single `impl Parser` block
**Target:** 3 focused modules + orchestrator

#### Module Breakdown

**2a. `frontend/parser/expression.rs`** (400 lines)
- `parse_expression()` - Main dispatcher
- `parse_prefix()` - Prefix operators
- `parse_infix()` - Infix operators
- `parse_if_expression()`
- `parse_match_expression()`
- `parse_function_literal()`
- `parse_array()`
- `parse_hash()`
- `parse_string()`
- `parse_interpolated_string()`
- `parse_integer()`
- `parse_float()`
- `parse_identifier()`
- `parse_call_expression()`
- `parse_index_expression()`
- `parse_member_access()`
- `parse_pipe_expression()`
- `parse_pattern()`
- All expression AST construction

**2b. `frontend/parser/statement.rs`** (150 lines)
- `parse_statement()` - Main dispatcher
- `parse_let_statement()`
- `parse_assignment_statement()`
- `parse_function_statement()`
- `parse_module_statement()`
- `parse_import_statement()`
- `parse_return_statement()`
- `parse_expression_statement()`
- All statement AST construction

**2c. `frontend/parser/utils.rs`** (50 lines)
- `next_token()` - Token navigation
- `is_peek_token()`
- `expect_peek()`
- `synchronize_after_error()`
- `span_from()`
- `current_precedence()`
- `peek_precedence()`
- `no_prefix_parse_error()`
- Helper functions

**Orchestrator:** `parser.rs` (refactored to ~200 lines)
- Parser struct
- Public API: `new()`, `parse_program()`, `parse_repl()`
- Module delegation
- Error collection

#### File Structure
```
src/frontend/
  ├── parser/
  │   ├── mod.rs       # Public exports, Parser struct
  │   ├── expression.rs # Expression parsing
  │   ├── statement.rs  # Statement parsing
  │   └── utils.rs      # Helper functions
  └── parser.rs        # DEPRECATED - re-exports for compatibility
```

#### Migration Strategy
1. Create `frontend/parser/` directory
2. Extract methods into new modules
3. Update parser.rs to delegate
4. Run parser tests after each module
5. Test with all examples

#### Estimated Effort
- 2 days
- Risk: Medium (core parsing logic)

---

### 3. VM.rs Split (HIGH PRIORITY)

**Current:** 824 lines, massive `run_inner()` method
**Target:** 6 focused modules + orchestrator

#### Module Breakdown

**3a. `runtime/vm/dispatch.rs`** (225 lines)
- `dispatch_instruction()` - Main opcode router
- Matches on 99 OpCode variants
- Delegates to specialized handlers
- Minimal logic, pure routing

**3b. `runtime/vm/binary_ops.rs`** (80 lines)
- `execute_binary_operation()`
- Integer arithmetic: +, -, *, /, %
- Float arithmetic: +, -, *, /
- Type coercion (int → float)
- Error handling for division by zero

**3c. `runtime/vm/comparison_ops.rs`** (150 lines)
- `execute_comparison()`
- Equality: ==, != (all types)
- Ordering: <, >, <=, >= (int, float, string)
- Type-specific comparison logic
- Boolean result handling

**3d. `runtime/vm/index_ops.rs`** (50 lines)
- `execute_index_expression()`
- `execute_array_index()`
- `execute_hash_index()`
- Bounds checking
- Error handling for invalid indices

**3e. `runtime/vm/function_call.rs`** (80 lines)
- `execute_call()`
- `call_closure()`
- `push_closure()`
- Frame management
- Argument handling
- Return value processing

**3f. `runtime/vm/trace.rs`** (60 lines)
- `trace_instruction()`
- `trace_stack()`
- `trace_locals()`
- `format_frame()`
- `format_runtime_error()`
- Debug output formatting
- Source line extraction

**Orchestrator:** `vm.rs` (refactored to ~150 lines)
- VM struct
- Public API: `new()`, `new_with_globals()`, `run()`, `last_popped()`
- Stack management (push/pop)
- Frame management (current_frame, push_frame)
- High-level execution loop

#### File Structure
```
src/runtime/
  ├── vm/
  │   ├── mod.rs           # Public exports, VM struct
  │   ├── dispatch.rs      # Instruction routing
  │   ├── binary_ops.rs    # Arithmetic operations
  │   ├── comparison_ops.rs # Comparison operations
  │   ├── index_ops.rs     # Array/hash indexing
  │   ├── function_call.rs # Function invocation
  │   └── trace.rs         # Debug/trace output
  └── vm.rs               # DEPRECATED - re-exports for compatibility
```

#### Migration Strategy
1. Create `runtime/vm/` directory
2. Extract operation handlers (start with trace, least critical)
3. Extract binary_ops and comparison_ops
4. Extract index_ops and function_call
5. Refactor dispatch.rs to route to handlers
6. Run VM tests after each extraction
7. Test with all examples and integration tests

#### Estimated Effort
- 3 days
- Risk: High (core runtime logic, frequent changes)

---

## Secondary Refactorings (Medium Priority)

### 4. Module_graph.rs Split

**Current:** 502 lines
**Target:** 3 modules + orchestrator

**Modules:**
- `module_resolution.rs` (150 lines) - Path resolution, validation
- `module_order.rs` (80 lines) - Topological sort, cycle detection
- `module_binding.rs` (30 lines) - Binding name utilities

**Estimated Effort:** 1-2 days

### 5. Builtins.rs Split

**Current:** 757 lines
**Target:** 6 modules + registry

**Modules:**
- `helpers.rs` (120 lines) - Validation + error formatting
- `array_ops.rs` (200 lines) - Array operations
- `string_ops.rs` (150 lines) - String operations
- `hash_ops.rs` (100 lines) - Hash operations
- `type_check.rs` (60 lines) - Type introspection
- `numeric_ops.rs` (60 lines) - Math operations

**Estimated Effort:** 2-3 days

### 6. Bytecode_cache.rs Split

**Current:** 481 lines
**Target:** 2 modules + orchestrator

**Modules:**
- `cache_serialization.rs` (180 lines) - Binary format
- `cache_validation.rs` (80 lines) - Version + hash checks

**Estimated Effort:** 1 day

---

## Testing Strategy

### Per-Module Tests
Each new module should include:
1. **Unit tests** - Test module functions in isolation
2. **Integration tests** - Verify module works with others
3. **Regression tests** - Ensure no behavior changes

### Validation Checklist
Before marking a split complete:
- [ ] All existing tests pass
- [ ] New module has test coverage
- [ ] Documentation updated
- [ ] Examples still run correctly
- [ ] Performance unchanged (or improved)

### Test Approach
1. **Before split:** Capture baseline test results
2. **During split:** Run tests after each module extraction
3. **After split:** Full regression suite (unit + integration + examples)

---

## Implementation Order

### Week 1: Compiler Split
**Days 1-3:** compiler.rs → 4 modules
- Day 1: Extract errors.rs + builder.rs
- Day 2: Extract statement.rs
- Day 3: Extract expression.rs, refactor orchestrator

**Deliverable:** Modular compiler with same API

### Week 2: Parser Split
**Days 4-5:** parser.rs → 3 modules
- Day 4: Extract utils.rs + statement.rs
- Day 5: Extract expression.rs, refactor orchestrator

**Deliverable:** Modular parser with same API

### Week 3: VM Split
**Days 6-8:** vm.rs → 6 modules
- Day 6: Extract trace.rs + index_ops.rs
- Day 7: Extract binary_ops.rs + comparison_ops.rs
- Day 8: Extract function_call.rs + dispatch.rs, refactor orchestrator

**Deliverable:** Modular VM with same API

### Week 4: Secondary Splits (Optional)
**Days 9-12:** Medium-priority splits
- Day 9: module_graph.rs split
- Day 10-11: builtins.rs split
- Day 12: bytecode_cache.rs split

**Deliverable:** Fully modularized codebase

---

## Success Metrics

### Code Quality
- **Largest file size:** < 400 lines (down from 1,671)
- **Average module size:** 100-200 lines
- **Single Responsibility:** Each module has one clear purpose

### Maintainability
- **Test isolation:** Each module independently testable
- **Documentation:** Each module has clear purpose/API docs
- **Discoverability:** Clear module organization

### Stability
- **Test coverage:** 100% of existing tests pass
- **Performance:** No regressions in benchmarks
- **Examples:** All examples run correctly

---

## Risks and Mitigation

### Risk 1: Breaking Changes
**Likelihood:** Low
**Impact:** High
**Mitigation:**
- Keep original files as re-export wrappers during transition
- Extensive regression testing
- Gradual migration path

### Risk 2: Performance Regression
**Likelihood:** Low
**Impact:** Medium
**Mitigation:**
- Benchmark before/after each split
- Profile hot paths (especially VM)
- Inline critical functions if needed

### Risk 3: Increased Complexity
**Likelihood:** Medium
**Impact:** Low
**Mitigation:**
- Clear module documentation
- Consistent naming conventions
- Module dependency diagrams

### Risk 4: Merge Conflicts
**Likelihood:** Medium (if concurrent work)
**Impact:** Medium
**Mitigation:**
- Communicate refactoring schedule
- Work on feature branches
- Merge frequently

---

## Future Considerations

### Post-Split Opportunities
Once modules are split, we can:
1. **Add module-specific tests** - Test expression compiler independently
2. **Parallel development** - Multiple people can work on different modules
3. **Gradual improvements** - Refactor modules without touching others
4. **Trait-based dispatch** - Consider trait-based VM dispatch (future)

### Phase 2 & 3 Alignment
This split prepares for:
- **Phase 2 (Desugaring):** Expression compiler becomes desugaring target
- **Phase 3 (Core IR):** Modular structure makes IR integration easier
- **Symbol Interning:** Easier to add once modules are focused

---

## References

- [Compiler Architecture](../architecture/compiler_architecture.md)
- [Symbol Interning Proposal](005_symbol_interning.md)
- Rust API Guidelines: [Module Organization](https://rust-lang.github.io/api-guidelines/organization.html)

---

## Approval Checklist

- [ ] Plan reviewed by maintainers
- [ ] Risk assessment complete
- [ ] Testing strategy approved
- [ ] Timeline agreed upon
- [ ] Ready to implement

---

**Next Steps:** Review plan, get approval, start with compiler.rs split.
