# Proposal 007: Visitor Pattern for Multi-Pass Compilation

**Status:** Planning
**Priority:** High (Enables future type system)
**Created:** 2026-02-01
**Depends on:** Phase 1 Module Split (Proposal 006)

## Overview

Introduce the Visitor pattern to enable clean multi-pass compilation, making it easy to add new compiler passes (type checking, optimization, linting) without modifying the AST or existing passes.

## Problem Statement

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

## Goals

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

## Proposed Solution

### 1. Core Visitor Trait

Create `src/frontend/visitor.rs`:

```rust
//! Generic visitor pattern for AST traversal.

use crate::frontend::{
    expression::Expression,
    statement::Statement,
    block::Block,
    program::Program,
};

/// Generic visitor trait for traversing AST nodes.
///
/// Type parameter `T` is the return type of visit methods.
/// Different visitors can return different types:
/// - TypeChecker returns `Type`
/// - Optimizer returns `Expression` (optimized)
/// - CodeGenerator returns `()` (just emits bytecode)
/// - Linter returns `Vec<Warning>`
pub trait Visitor<T = ()> {
    type Error;

    // ===== Expression Visitors =====

    /// Visit any expression (dispatcher).
    fn visit_expression(&mut self, expr: &Expression) -> Result<T, Self::Error> {
        self.walk_expression(expr)
    }

    // Primitive expressions
    fn visit_integer(&mut self, value: i64, span: &Span) -> Result<T, Self::Error>;
    fn visit_float(&mut self, value: f64, span: &Span) -> Result<T, Self::Error>;
    fn visit_string(&mut self, value: &str, span: &Span) -> Result<T, Self::Error>;
    fn visit_boolean(&mut self, value: bool, span: &Span) -> Result<T, Self::Error>;
    fn visit_none(&mut self, span: &Span) -> Result<T, Self::Error>;
    fn visit_identifier(&mut self, name: &str, span: &Span) -> Result<T, Self::Error>;

    // Compound expressions
    fn visit_binary_op(
        &mut self,
        left: &Expression,
        operator: &str,
        right: &Expression,
        span: &Span,
    ) -> Result<T, Self::Error>;

    fn visit_unary_op(
        &mut self,
        operator: &str,
        right: &Expression,
        span: &Span,
    ) -> Result<T, Self::Error>;

    fn visit_if_expression(
        &mut self,
        condition: &Expression,
        consequence: &Block,
        alternative: &Option<Block>,
        span: &Span,
    ) -> Result<T, Self::Error>;

    fn visit_match_expression(
        &mut self,
        value: &Expression,
        arms: &[MatchArm],
        span: &Span,
    ) -> Result<T, Self::Error>;

    fn visit_function_literal(
        &mut self,
        parameters: &[String],
        body: &Block,
        span: &Span,
    ) -> Result<T, Self::Error>;

    fn visit_call(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
        span: &Span,
    ) -> Result<T, Self::Error>;

    fn visit_array(&mut self, elements: &[Expression], span: &Span) -> Result<T, Self::Error>;

    fn visit_hash(
        &mut self,
        pairs: &[(Expression, Expression)],
        span: &Span,
    ) -> Result<T, Self::Error>;

    fn visit_index(
        &mut self,
        left: &Expression,
        index: &Expression,
        span: &Span,
    ) -> Result<T, Self::Error>;

    fn visit_member_access(
        &mut self,
        object: &Expression,
        member: &str,
        span: &Span,
    ) -> Result<T, Self::Error>;

    fn visit_pipe(
        &mut self,
        left: &Expression,
        right: &Expression,
        span: &Span,
    ) -> Result<T, Self::Error>;

    // ===== Statement Visitors =====

    /// Visit any statement (dispatcher).
    fn visit_statement(&mut self, stmt: &Statement) -> Result<T, Self::Error> {
        self.walk_statement(stmt)
    }

    fn visit_let(
        &mut self,
        name: &str,
        value: &Expression,
        is_mutable: bool,
        span: &Span,
    ) -> Result<T, Self::Error>;

    fn visit_assign(
        &mut self,
        target: &Expression,
        value: &Expression,
        span: &Span,
    ) -> Result<T, Self::Error>;

    fn visit_return(&mut self, value: &Expression, span: &Span) -> Result<T, Self::Error>;

    fn visit_expression_statement(
        &mut self,
        expr: &Expression,
        span: &Span,
    ) -> Result<T, Self::Error>;

    fn visit_function(
        &mut self,
        name: &str,
        parameters: &[String],
        body: &Block,
        span: &Span,
    ) -> Result<T, Self::Error>;

    fn visit_module(
        &mut self,
        name: &str,
        body: &Block,
        span: &Span,
    ) -> Result<T, Self::Error>;

    fn visit_import(&mut self, path: &str, alias: &Option<String>, span: &Span) -> Result<T, Self::Error>;

    // ===== Block & Program Visitors =====

    fn visit_block(&mut self, block: &Block) -> Result<Vec<T>, Self::Error> {
        let mut results = Vec::new();
        for stmt in &block.statements {
            results.push(self.visit_statement(stmt)?);
        }
        Ok(results)
    }

    fn visit_program(&mut self, program: &Program) -> Result<Vec<T>, Self::Error> {
        let mut results = Vec::new();
        for stmt in &program.statements {
            results.push(self.visit_statement(stmt)?);
        }
        Ok(results)
    }

    // ===== Default Traversal (can be overridden) =====

    /// Default expression traversal - dispatches to specific visit_* methods.
    fn walk_expression(&mut self, expr: &Expression) -> Result<T, Self::Error> {
        match expr {
            Expression::Integer { value, span } => {
                self.visit_integer(*value, span)
            }
            Expression::Float { value, span } => {
                self.visit_float(*value, span)
            }
            Expression::String { value, span } => {
                self.visit_string(value, span)
            }
            Expression::Boolean { value, span } => {
                self.visit_boolean(*value, span)
            }
            Expression::None { span } => {
                self.visit_none(span)
            }
            Expression::Identifier { name, span } => {
                self.visit_identifier(name, span)
            }
            Expression::Infix { left, operator, right, span } => {
                self.visit_binary_op(left, operator, right, span)
            }
            Expression::Prefix { operator, right, span } => {
                self.visit_unary_op(operator, right, span)
            }
            Expression::If { condition, consequence, alternative, span } => {
                self.visit_if_expression(condition, consequence, alternative, span)
            }
            Expression::Match { value, arms, span } => {
                self.visit_match_expression(value, arms, span)
            }
            Expression::FunctionLiteral { parameters, body, span } => {
                self.visit_function_literal(parameters, body, span)
            }
            Expression::Call { function, arguments, span } => {
                self.visit_call(function, arguments, span)
            }
            Expression::Array { elements, span } => {
                self.visit_array(elements, span)
            }
            Expression::Hash { pairs, span } => {
                self.visit_hash(pairs, span)
            }
            Expression::Index { left, index, span } => {
                self.visit_index(left, index, span)
            }
            Expression::MemberAccess { object, member, span } => {
                self.visit_member_access(object, member, span)
            }
            Expression::Pipe { left, right, span } => {
                self.visit_pipe(left, right, span)
            }
            _ => unimplemented!("walk_expression for {:?}", expr),
        }
    }

    /// Default statement traversal - dispatches to specific visit_* methods.
    fn walk_statement(&mut self, stmt: &Statement) -> Result<T, Self::Error> {
        match stmt {
            Statement::Let { name, value, is_mutable, span } => {
                self.visit_let(name, value, *is_mutable, span)
            }
            Statement::Assign { target, value, span } => {
                self.visit_assign(target, value, span)
            }
            Statement::Return { value, span } => {
                self.visit_return(value, span)
            }
            Statement::Expression { expression, span } => {
                self.visit_expression_statement(expression, span)
            }
            Statement::Function { name, parameters, body, span } => {
                self.visit_function(name, parameters, body, span)
            }
            Statement::Module { name, body, span } => {
                self.visit_module(name, body, span)
            }
            Statement::Import { path, alias, span } => {
                self.visit_import(path, alias, span)
            }
            _ => unimplemented!("walk_statement for {:?}", stmt),
        }
    }
}
```

---

### 2. Example Visitors

#### 2.1 Type Checker (Future)

**File:** `src/frontend/type_checker.rs`

```rust
//! Type checking visitor for static type analysis.

use std::collections::HashMap;
use crate::frontend::{visitor::Visitor, expression::Expression};

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

#### 2.2 Constant Folder (Optimizer)

**File:** `src/bytecode/optimizer.rs`

```rust
//! Optimization visitor for constant folding and dead code elimination.

use crate::frontend::{visitor::Visitor, expression::Expression};

pub struct ConstantFolder {
    optimizations: usize,
}

impl Visitor<Expression> for ConstantFolder {
    type Error = ();

    fn visit_integer(&mut self, value: i64, span: &Span) -> Result<Expression, ()> {
        Ok(Expression::Integer { value, span: span.clone() })
    }

    fn visit_binary_op(
        &mut self,
        left: &Expression,
        operator: &str,
        right: &Expression,
        span: &Span,
    ) -> Result<Expression, ()> {
        // Recursively optimize sub-expressions
        let left = self.visit_expression(left)?;
        let right = self.visit_expression(right)?;

        // Constant folding
        match (&left, operator, &right) {
            (Expression::Integer { value: a, .. }, "+", Expression::Integer { value: b, .. }) => {
                self.optimizations += 1;
                Ok(Expression::Integer {
                    value: a + b,
                    span: span.clone(),
                })
            }
            (Expression::Integer { value: a, .. }, "*", Expression::Integer { value: b, .. }) => {
                self.optimizations += 1;
                Ok(Expression::Integer {
                    value: a * b,
                    span: span.clone(),
                })
            }
            // Identity: x + 0 = x
            (_, "+", Expression::Integer { value: 0, .. }) => {
                self.optimizations += 1;
                Ok(left)
            }
            // No optimization
            _ => Ok(Expression::Infix {
                left: Box::new(left),
                operator: operator.to_string(),
                right: Box::new(right),
                span: span.clone(),
            }),
        }
    }

    // ... other methods
}
```

#### 2.3 Code Generator

**File:** `src/bytecode/code_generator.rs`

```rust
//! Code generation visitor for bytecode emission.

use crate::{
    bytecode::{compiler::Compiler, op_code::OpCode},
    frontend::{visitor::Visitor, expression::Expression},
    runtime::object::Object,
};

pub struct CodeGenerator<'a> {
    compiler: &'a mut Compiler,
}

impl<'a> Visitor<()> for CodeGenerator<'a> {
    type Error = Box<Diagnostic>;

    fn visit_integer(&mut self, value: i64, _span: &Span) -> Result<(), Self::Error> {
        let idx = self.compiler.add_constant(Object::Integer(value));
        self.compiler.emit(OpCode::Constant, &[idx]);
        Ok(())
    }

    fn visit_binary_op(
        &mut self,
        left: &Expression,
        operator: &str,
        right: &Expression,
        _span: &Span,
    ) -> Result<(), Self::Error> {
        self.visit_expression(left)?;
        self.visit_expression(right)?;

        match operator {
            "+" => self.compiler.emit(OpCode::Add, &[]),
            "-" => self.compiler.emit(OpCode::Sub, &[]),
            "*" => self.compiler.emit(OpCode::Mul, &[]),
            "/" => self.compiler.emit(OpCode::Div, &[]),
            _ => return Err(/* unknown operator */),
        };

        Ok(())
    }

    // ... other methods
}
```

---

## Implementation Plan

### Phase 1: Foundation (Week 1)

**Goal:** Create visitor infrastructure without breaking existing code

**Tasks:**
1. Create `src/frontend/visitor.rs` with Visitor trait
2. Add tests for visitor trait
3. Document visitor pattern usage
4. No changes to existing compiler yet

**Deliverables:**
- [ ] Visitor trait with all expression/statement methods
- [ ] Documentation with examples
- [ ] Test framework for visitors

---

### Phase 2: Example Visitors (Week 2)

**Goal:** Prove visitor pattern works with concrete examples

**Tasks:**
1. Implement ConstantFolder visitor
2. Implement ASTPrinter visitor (for debugging)
3. Implement basic Linter visitor
4. Test each visitor independently

**Deliverables:**
- [ ] Working ConstantFolder (optimization pass)
- [ ] ASTPrinter (debug utility)
- [ ] Linter (code quality checks)
- [ ] Unit tests for each visitor

---

### Phase 3: Code Generator Migration (Week 3-4)

**Goal:** Migrate code generation to use visitor pattern

**Tasks:**
1. Create CodeGenerator visitor
2. Run in parallel with existing compile_expression
3. Compare outputs (must be identical)
4. Switch to visitor-based when validated
5. Remove old compile_expression

**Deliverables:**
- [ ] CodeGenerator visitor matches old behavior
- [ ] All tests pass
- [ ] Benchmarks show no regression

---

### Phase 4: Multi-Pass Pipeline (Week 5)

**Goal:** Enable configurable compilation pipeline

**Tasks:**
1. Create CompilerPipeline abstraction
2. Support pass registration
3. Enable optional passes
4. Add pass timing/profiling

**Example:**
```rust
let mut pipeline = CompilerPipeline::new();
pipeline.add_pass(TypeChecker::new());
pipeline.add_pass(ConstantFolder::new());
pipeline.add_pass(CodeGenerator::new(&mut compiler));

pipeline.run(&program)?;
```

**Deliverables:**
- [ ] Pipeline infrastructure
- [ ] Configurable passes
- [ ] Performance profiling

---

## Migration Strategy

### Backward Compatibility

**Keep existing code during migration:**

```rust
// Compiler can work both ways during transition
impl Compiler {
    pub fn compile(&mut self, program: &Program) -> Result<Bytecode> {
        if self.use_visitor_pattern {
            self.compile_with_visitor(program)
        } else {
            self.compile_legacy(program)
        }
    }

    fn compile_with_visitor(&mut self, program: &Program) -> Result<Bytecode> {
        let mut codegen = CodeGenerator::new(self);
        codegen.visit_program(program)?;
        Ok(self.bytecode())
    }

    fn compile_legacy(&mut self, program: &Program) -> Result<Bytecode> {
        // Existing compile logic
        // ...
    }
}
```

### Feature Flag

```rust
// Enable visitor pattern with feature flag
#[cfg(feature = "visitor-pattern")]
const USE_VISITOR: bool = true;

#[cfg(not(feature = "visitor-pattern"))]
const USE_VISITOR: bool = false;
```

### Validation

```rust
#[cfg(test)]
mod migration_tests {
    #[test]
    fn visitor_matches_legacy() {
        let source = "1 + 2 * 3";

        let bytecode_legacy = compile_legacy(source);
        let bytecode_visitor = compile_with_visitor(source);

        assert_eq!(bytecode_legacy, bytecode_visitor);
    }
}
```

---

## Benefits

### Immediate Benefits
1. ✅ **Cleaner code organization** - Each pass in its own file
2. ✅ **Independent testing** - Test each pass separately
3. ✅ **Easier debugging** - Clear which pass has the bug
4. ✅ **Parallel development** - Multiple people can work on different passes

### Future Benefits
1. ✅ **Type system** - Clean type checking pass
2. ✅ **Optimization** - Multiple optimization passes
3. ✅ **Linting** - Code quality checks
4. ✅ **IDE support** - Separate analysis passes for IDE features
5. ✅ **Incremental compilation** - Cache pass results

---

## Testing Strategy

### Unit Tests for Each Visitor

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_folding() {
        let mut folder = ConstantFolder::new();
        let expr = parse("1 + 2");
        let result = folder.visit_expression(&expr).unwrap();

        assert_eq!(result, Expression::Integer { value: 3, .. });
    }

    #[test]
    fn test_type_checking() {
        let mut checker = TypeChecker::new();
        let expr = parse("1 + 2.5");
        let result = checker.visit_expression(&expr).unwrap();

        assert_eq!(result, Type::Float);
    }
}
```

### Integration Tests

```rust
#[test]
fn test_multi_pass_pipeline() {
    let source = "let x = 1 + 2; x * 3";

    let program = parse(source);

    // Pass 1: Optimize
    let mut folder = ConstantFolder::new();
    let optimized = folder.visit_program(&program).unwrap();

    // Pass 2: Generate code
    let mut compiler = Compiler::new();
    let mut codegen = CodeGenerator::new(&mut compiler);
    codegen.visit_program(&optimized).unwrap();

    // Verify output
    let bytecode = compiler.bytecode();
    assert!(bytecode.instructions.len() > 0);
}
```

---

## Success Metrics

### Code Quality
- [ ] Each visitor < 300 lines
- [ ] Clear separation of concerns
- [ ] No code duplication between passes

### Testing
- [ ] 100% of existing tests pass
- [ ] Each visitor has independent unit tests
- [ ] Integration tests for multi-pass pipeline

### Performance
- [ ] No regression in compilation speed
- [ ] Benchmark visitor vs direct pattern matching
- [ ] Profile pass overhead

---

## Risks and Mitigation

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

---

## Future Opportunities

### Post-Visitor Features

Once visitor pattern is in place:

1. **Type System** (Proposal 008)
   - Static type checking
   - Type inference
   - Generic types

2. **Advanced Optimizations**
   - Dead code elimination
   - Constant propagation
   - Inlining

3. **Language Server Protocol (LSP)**
   - Hover information (type checker)
   - Code completion (type checker)
   - Diagnostics (linter)

4. **Incremental Compilation**
   - Cache pass results
   - Only rerun changed passes
   - Faster recompilation

---

## References

- [Visitor Pattern (Gang of Four)](https://en.wikipedia.org/wiki/Visitor_pattern)
- [Crafting Interpreters - Visitor Pattern](https://craftinginterpreters.com/representing-code.html#the-visitor-pattern)
- [Rust Compiler - Visitor Infrastructure](https://doc.rust-lang.org/nightly/nightly-rustc/rustc_ast/visit/index.html)
- [LLVM Pass Infrastructure](https://llvm.org/docs/WritingAnLLVMPass.html)

---

## Approval Checklist

- [ ] Design reviewed
- [ ] Migration strategy approved
- [ ] Testing plan validated
- [ ] Performance benchmarks defined
- [ ] Ready to implement

---

**Next Steps:**
1. Review and approve proposal
2. Start Phase 1 (visitor infrastructure)
3. Implement example visitors
4. Gradually migrate existing code
