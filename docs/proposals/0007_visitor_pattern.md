- Feature Name: Visitor Pattern for Multi-Pass Compilation
- Start Date: 2026-02-01
- Proposal PR: 
- Flux Issue: 

# Proposal 0007: Visitor Pattern for Multi-Pass Compilation

## Summary
[summary]: #summary

Introduce the Visitor pattern to enable clean multi-pass compilation, making it easy to add new compiler passes (type checking, optimization, linting) without modifying the AST or existing passes.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Primary Goals

1. Enable multi-pass compilation (type check, optimize, generate)
2. Separate concerns (each pass in its own module)
3. Make adding new passes easy (no changes to existing code)
4. Enable independent testing of each pass

### Secondary Goals

1. Maintain backward compatibility (gradual migration)
2. Improve code organization (smaller, focused modules)
3. Enable optional passes (skip optimization for debug builds)
4. Support future type system (separate type checking pass)

### Primary Goals

### Secondary Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **1. Core Visitor Trait:** use crate::syntax::{ expression::Expression, statement::Statement, block::Block, program::Program, }; - **2.2 Constant Folder (Optimizer):** use cra...
- **1. Core Visitor Trait:** Create `src/syntax/visitor.rs`: ```rust //! Generic visitor pattern for AST traversal.
- **2.2 Constant Folder (Optimizer):** use crate::syntax::{visitor::Visitor, expression::Expression};
- **2.3 Code Generator:** use crate::{ bytecode::{compiler::Compiler, op_code::OpCode}, syntax::{visitor::Visitor, expression::Expression}, runtime::object::Object, };
- **Phase 1: Foundation (Week 1):** **Goal:** Create visitor infrastructure without breaking existing code
- **Phase 2: Example Visitors (Week 2):** **Goal:** Prove visitor pattern works with concrete examples

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### Current Architecture Limitations

**1. Mixed Concerns in Compiler**
```rust
// compiler.rs - Everything mixed together
impl Compiler {
    fn compile_expression(&mut self, expr: &Expression) -> Result<()> {
        match expr {
            Expression::Infix { left, operator, right, .. } => {
                // Code generation
                self.compile_expression(left)?;
                self.compile_expression(right)?;
                self.emit(OpCode::Add, &[]);

                // Where would type checking go?
                // Where would optimization go?
                // All mixed together!
            }
        }
    }
}
```

**2. Adding New Passes Requires Code Duplication**
- Want to add type checking? Duplicate all pattern matching
- Want to add optimization? Duplicate again
- Each new pass = 500+ lines of duplicated traversal code

**3. Hard to Test Passes in Isolation**
- Can't test type checking without running code generation
- Can't test optimization without running type checking
- Integration tests only, no unit tests for individual passes

**4. Blocks Future Features**
- Type system requires separate type checking pass
- Optimization requires separate optimization pass
- Linting requires separate linting pass
- Current architecture makes these very difficult

### Risk 1: Performance Overhead

**Likelihood:** Medium
**Impact:** Low
**Mitigation:**
- Benchmark visitor vs direct calls
- Inline critical paths
- Use zero-cost abstractions

### Risk 2: Complexity

**Likelihood:** Medium
**Impact:** Medium
**Mitigation:**
- Comprehensive documentation
- Examples for each visitor type
- Clear migration guide

### Risk 3: Breaking Changes

**Likelihood:** Low
**Impact:** High
**Mitigation:**
- Gradual migration with feature flags
- Keep existing code during transition
- Extensive testing

### Current Architecture Limitations

### Risk 1: Performance Overhead

### Risk 2: Complexity

### Risk 3: Breaking Changes

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### References

- [Visitor Pattern (Gang of Four)](https://en.wikipedia.org/wiki/Visitor_pattern)
- [Crafting Interpreters - Visitor Pattern](https://craftinginterpreters.com/representing-code.html#the-visitor-pattern)
- [Rust Compiler - Visitor Infrastructure](https://doc.rust-lang.org/nightly/nightly-rustc/rustc_ast/visit/index.html)
- [LLVM Pass Infrastructure](https://llvm.org/docs/WritingAnLLVMPass.html)

### References

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

### 2.1 Type Checker (Future)

**File:** `src/syntax/type_checker.rs`

```rust
//! Type checking visitor for static type analysis.

use std::collections::HashMap;
use crate::syntax::{visitor::Visitor, expression::Expression};

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Int,
    Float,
    String,
    Bool,
    None,
    Function { params: Vec<Type>, returns: Box<Type> },
    Array(Box<Type>),
    Hash { key: Box<Type>, value: Box<Type> },
}

pub struct TypeChecker {
    env: HashMap<String, Type>,
    errors: Vec<TypeError>,
}

impl Visitor<Type> for TypeChecker {
    type Error = ();

    fn visit_integer(&mut self, _value: i64, _span: &Span) -> Result<Type, ()> {
        Ok(Type::Int)
    }

    fn visit_binary_op(
        &mut self,
        left: &Expression,
        operator: &str,
        right: &Expression,
        _span: &Span,
    ) -> Result<Type, ()> {
        let left_type = self.visit_expression(left)?;
        let right_type = self.visit_expression(right)?;

        match operator {
            "+" | "-" | "*" | "/" => {
                match (&left_type, &right_type) {
                    (Type::Int, Type::Int) => Ok(Type::Int),
                    (Type::Float, Type::Float) => Ok(Type::Float),
                    _ => {
                        self.errors.push(TypeError::TypeMismatch {
                            expected: left_type.clone(),
                            got: right_type,
                        });
                        Ok(Type::Int) // Error recovery
                    }
                }
            }
            _ => Ok(Type::Bool),
        }
    }

    // ... other methods
}
```

### Future Benefits

1. ✅ **Type system** - Clean type checking pass
2. ✅ **Optimization** - Multiple optimization passes
3. ✅ **Linting** - Code quality checks
4. ✅ **IDE support** - Separate analysis passes for IDE features
5. ✅ **Incremental compilation** - Cache pass results

### 2.1 Type Checker (Future)

**File:** `src/syntax/type_checker.rs`

### Future Benefits
