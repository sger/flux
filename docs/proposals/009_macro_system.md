# Proposal 009: Macro System for Flux

**Status:** Planning
**Priority:** High (Architectural Foundation)
**Created:** 2026-02-02
**Influences:** Elixir, Lisp, Rust

## Overview

This proposal introduces a **macro system** to Flux, enabling compile-time code generation and transformation. Following Elixir's philosophy of "put power in macros — keep the compiler small," this system would allow moving language features, builtins, and control flow from the compiler into user-space code.

## Motivation: The Elixir Insight

### Current Problem

Flux's compiler must understand every language feature:
- Built-in functions (35 hard-coded in VM)
- Control flow (if/else hard-coded in compiler)
- Future features require compiler changes

**Result:** Growing compiler complexity, hard to extend.

### Elixir's Solution

**Macro-first design:**
```elixir
# These LOOK like syntax but are actually macros
unless condition do
  body
end

# Expands to:
if not(condition) do
  body
end

# Even 'if' itself is a macro!
defmacro if(condition, do: do_clause, else: else_clause) do
  quote do
    case unquote(condition) do
      x when x in [false, nil] -> unquote(else_clause)
      _ -> unquote(do_clause)
    end
  end
end
```

**Key insight:** Only `case` and pattern matching are truly compiler features. Everything else is macros.

### What This Means for Flux

**With macros, we can:**
1. Move 20+ builtins to standard library (macro-generated)
2. Let users define control flow (unless, until, guard, etc.)
3. Build DSLs (SQL, HTML, testing frameworks)
4. Keep compiler focused on core semantics

## Macro System Design

### 1. Core Concepts

#### A. Quote: Capture Code as Data
```flux
// Syntax (to be designed)
let ast = quote {
    let x = 1 + 2;
    print(x);
};

// ast is now a data structure representing the code
// Type: Expr::Block(...)
```

#### B. Unquote: Inject Data into Code
```flux
macro make_adder(n) {
    quote {
        fun (x) { x + unquote(n) }
    }
}

// Expands to:
make_adder(5)
// => fun (x) { x + 5 }
```

#### C. Macro Expansion (Compile-Time)
```flux
macro unless(condition, body) {
    quote {
        if !(unquote(condition)) {
            unquote(body)
        }
    }
}

// Usage:
unless(x > 10) {
    print("x is small");
}

// Compiler expands to:
if !(x > 10) {
    print("x is small");
}
```

### 2. Macro Syntax (Proposal)

#### Defining Macros
```flux
// Basic macro
macro name(arg1, arg2) {
    quote {
        // Code template with unquote for injection
    }
}

// Example: Custom control flow
macro when(condition, body) {
    quote {
        if (unquote(condition)) {
            unquote(body)
        }
    }
}
```

#### Using Macros
```flux
// Looks like normal function call
when(x > 5) {
    print("x is big");
}

// But expands at compile-time before VM execution
```

### 3. AST Representation

Macros operate on Flux's AST. We need to expose AST as first-class values:

```rust
// In Flux runtime
pub enum Expr {
    Integer(i64),
    Binary { left: Box<Expr>, op: String, right: Box<Expr> },
    If { condition: Box<Expr>, consequence: Block, alternative: Option<Block> },
    Call { function: Box<Expr>, arguments: Vec<Expr> },
    Macro { name: String, arguments: Vec<Expr> },  // NEW
    Quote(Box<Expr>),                              // NEW
    Unquote(Box<Expr>),                            // NEW
    // ... existing variants
}
```

### 4. Expansion Pipeline

```
Source Code
    ↓
Parse to AST
    ↓
┌─────────────────┐
│ MACRO EXPANSION │ ← NEW PHASE
│                 │
│ 1. Find macro calls
│ 2. Execute macro function
│ 3. Replace call with result
│ 4. Recurse until no macros
└─────────────────┘
    ↓
Type Checking (future)
    ↓
Compilation to Bytecode
    ↓
VM Execution
```

## Implementation Strategy

### Phase 1: AST as Data (Week 1-2)

#### 1.1 Add Quote/Unquote to AST
```rust
// frontend/expression.rs
pub enum Expression {
    // ... existing variants
    Quote(Box<Expression>),
    Unquote(Box<Expression>),
}
```

#### 1.2 Parse Quote/Unquote
```rust
// frontend/parser.rs
fn parse_quote_expression(&mut self) -> Expression {
    self.expect_token(Token::Quote)?;
    let expr = self.parse_block_or_expression()?;
    Expression::Quote(Box::new(expr))
}
```

#### 1.3 AST as Runtime Object
```rust
// runtime/object.rs
pub enum Object {
    // ... existing variants
    Ast(Box<Expression>),  // NEW: AST as first-class value
}
```

### Phase 2: Macro Definition (Week 3-4)

#### 2.1 Parse Macro Definitions
```rust
// frontend/statement.rs
pub struct MacroStatement {
    pub name: String,
    pub parameters: Vec<String>,
    pub body: Expression,  // Usually a Quote expression
}
```

#### 2.2 Store Macros in Environment
```rust
// bytecode/compiler.rs
pub struct Compiler {
    // ... existing fields
    macros: HashMap<String, MacroDefinition>,  // NEW
}

pub struct MacroDefinition {
    pub name: String,
    pub params: Vec<String>,
    pub body: Expression,
}
```

### Phase 3: Macro Expansion (Week 5-6)

#### 3.1 Create Macro Expander
```rust
// frontend/macro_expander.rs
pub struct MacroExpander {
    macros: HashMap<String, MacroDefinition>,
}

impl MacroExpander {
    pub fn expand(&mut self, expr: Expression) -> Result<Expression, String> {
        match expr {
            Expression::Call { function, arguments } => {
                if let Expression::Identifier(name) = *function {
                    if let Some(macro_def) = self.macros.get(&name) {
                        return self.expand_macro(macro_def, arguments);
                    }
                }
                // Not a macro, recurse on children
                self.expand_call(function, arguments)
            }
            // Recurse into other expression types
            _ => self.expand_children(expr),
        }
    }

    fn expand_macro(
        &mut self,
        macro_def: &MacroDefinition,
        args: Vec<Expression>,
    ) -> Result<Expression, String> {
        // 1. Bind macro parameters to argument ASTs
        let env = self.bind_params(&macro_def.params, &args);

        // 2. Evaluate macro body (interpret at compile-time)
        let result_ast = self.eval_macro_body(&macro_def.body, &env)?;

        // 3. Recurse to expand nested macros
        self.expand(result_ast)
    }
}
```

#### 3.2 Integrate into Compiler
```rust
// bytecode/compiler.rs
impl Compiler {
    pub fn compile(&mut self, program: Program) -> Result<Bytecode, Box<Diagnostic>> {
        // NEW: Expand macros before compilation
        let mut expander = MacroExpander::new();
        let expanded_program = expander.expand_program(program)?;

        // Continue with normal compilation
        for statement in expanded_program.statements {
            self.compile_statement(&statement)?;
        }
        // ...
    }
}
```

### Phase 4: Quote/Unquote Evaluation (Week 7-8)

#### 4.1 Evaluate Quote (Return AST as Object)
```rust
// bytecode/compiler.rs
fn compile_quote_expression(&mut self, expr: &Expression) -> Result<(), Box<Diagnostic>> {
    // Convert AST to runtime Object::Ast
    let ast_object = self.ast_to_object(expr)?;

    // Emit instruction to load this constant
    let const_index = self.add_constant(ast_object);
    self.emit(OpCode::Constant, &[const_index]);

    Ok(())
}
```

#### 4.2 Evaluate Unquote (Inject Value)
```rust
// During macro expansion, unquote evaluates its expression
fn eval_unquote(&mut self, expr: &Expression, env: &Environment) -> Expression {
    // Evaluate the unquoted expression in the current environment
    let value = self.eval_expression(expr, env)?;

    // Convert value back to AST node
    self.value_to_ast(value)
}
```

## Examples: What Becomes Possible

### Example 1: Custom Control Flow

**Without Macros (Must Add to Compiler):**
```rust
// Would need to modify parser, compiler, VM
// Add Token::Unless, parse_unless_expression(), compile_unless(), OpUnless
```

**With Macros (Pure Flux Code):**
```flux
// In stdlib/control.flx
module Stdlib.Control {
    macro unless(condition, body) {
        quote {
            if !(unquote(condition)) {
                unquote(body)
            }
        }
    }

    macro until(condition, body) {
        quote {
            while !(unquote(condition)) {
                unquote(body)
            }
        }
    }
}

// Usage:
import Stdlib.Control

unless(user.is_admin()) {
    return "Access denied";
}
```

### Example 2: Move Builtins to Stdlib

**Current (Hard-coded in VM):**
```rust
// runtime/builtins.rs
fn builtin_is_int(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_int", "is_int(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::Integer(_))))
}
```

**With Macros (Pure Flux):**
```flux
// stdlib/type.flx
module Stdlib.Type {
    // type_of() remains a VM builtin
    // Everything else is a macro

    macro is_int(x) {
        quote {
            type_of(unquote(x)) == "Integer"
        }
    }

    macro is_array(x) {
        quote {
            type_of(unquote(x)) == "Array"
        }
    }

    macro is_string(x) {
        quote {
            type_of(unquote(x)) == "String"
        }
    }
}

// Usage:
import Stdlib.Type

if is_int(x) {
    print("x is an integer");
}

// Expands to:
if type_of(x) == "Integer" {
    print("x is an integer");
}
```

### Example 3: Assert Macro (Testing DSL)

```flux
// stdlib/test.flx
module Stdlib.Test {
    macro assert(condition) {
        quote {
            if !(unquote(condition)) {
                print("Assertion failed: " + stringify(unquote(condition)));
                // Could even capture source location if we expose that
            }
        }
    }

    macro assert_eq(left, right) {
        quote {
            let _left = unquote(left);
            let _right = unquote(right);
            if _left != _right {
                print("Assertion failed:");
                print("  Expected: " + to_string(_right));
                print("  Got: " + to_string(_left));
            }
        }
    }
}

// Usage:
import Stdlib.Test

fun test_addition() {
    assert_eq(2 + 2, 4);
    assert(result > 0);
}
```

### Example 4: Lazy Evaluation

```flux
// stdlib/lazy.flx
module Stdlib.Lazy {
    macro or_else(value, default) {
        quote {
            let _val = unquote(value);
            if _val == none {
                unquote(default)
            } else {
                _val
            }
        }
    }
}

// Usage:
import Stdlib.Lazy

let result = find(arr, key) or_else compute_default();

// Expands to:
let _val = find(arr, key);
let result = if _val == none {
    compute_default()  // Only called if find returns none!
} else {
    _val
};
```

### Example 5: Pipeline Operator (Elixir-style)

```flux
// stdlib/pipeline.flx
module Stdlib.Pipeline {
    macro pipe(value, function) {
        quote {
            unquote(function)(unquote(value))
        }
    }
}

// Usage:
import Stdlib.Pipeline

let result = data
    |> parse()
    |> validate()
    |> transform();

// Expands to:
let result = transform(validate(parse(data)));
```

## Reduced Builtin Set

With macros, we can drastically reduce VM builtins:

### Must Stay as VM Builtins (Core Set: ~10 functions)

```
Data introspection:
  - len()              # VM knows internal representation
  - type_of()          # VM-level type information

Array primitives:
  - first()            # O(1) access
  - push()             # Efficient mutation

Hash primitives:
  - keys()             # VM knows hash structure
  - values()           # VM knows hash structure

I/O:
  - print()            # Requires VM support

String primitives:
  - chars()            # UTF-8 handling

Numeric:
  - abs()              # Could be macro, but performance
```

### Move to Macro-Based Stdlib (~25 functions)

```flux
// All type checking
is_int, is_float, is_string, is_bool, is_array, is_hash, is_none, is_some

// Array operations
last, rest, reverse, sort, concat, slice

// String operations
upper, lower, trim, contains, join, split, substring

// Hash operations
has_key, merge

// Numeric
min, max (could be macros comparing two values)

// Utility
to_string (could be macro calling type-specific converters)
```

**Result:** 35 builtins → 10 core builtins + 25 macro-based functions

## Benefits

### 1. Smaller, Simpler Compiler
- Only needs to understand core semantics
- Control flow can be macros (unless, until, guard, etc.)
- Type checking becomes macros (is_int, is_array, etc.)

### 2. User Extensibility
- Users can define their own control flow
- DSLs for testing, HTML, SQL, etc.
- No need to modify compiler

### 3. Better Error Messages
```flux
macro assert_positive(x) {
    quote {
        if unquote(x) <= 0 {
            error("Expected positive value, got: " + to_string(unquote(x)));
        }
    }
}

// Expands with source location information preserved
```

### 4. Performance Opportunities
- Macros expand at compile-time (zero runtime cost)
- Can inline aggressively
- Compiler can optimize expanded code

### 5. Faster Language Evolution
- Add features without compiler changes
- Experiment with new syntax in libraries
- Community can contribute extensions

## Trade-offs and Challenges

### Challenges

#### 1. Hygiene Problem
```flux
// Bad: Variable capture
macro swap(a, b) {
    quote {
        let temp = unquote(a);
        unquote(a) = unquote(b);
        unquote(b) = temp;
    }
}

// Problem: What if user has a variable named 'temp'?
let temp = 10;
swap(x, y);  // Accidentally overwrites user's 'temp'!
```

**Solution:** Gensym (generate unique symbols)
```flux
macro swap(a, b) {
    let temp_var = gensym("temp");  // Generate unique name
    quote {
        let unquote(temp_var) = unquote(a);
        unquote(a) = unquote(b);
        unquote(b) = unquote(temp_var);
    }
}
```

#### 2. Debugging Expanded Code
- Stack traces show expanded code, not original macro
- Need source map to trace back to macro invocation

**Solution:** Preserve source location metadata
```rust
pub struct Expression {
    pub kind: ExpressionKind,
    pub span: Span,
    pub macro_expansion: Option<MacroExpansionInfo>,  // NEW
}

pub struct MacroExpansionInfo {
    pub macro_name: String,
    pub invocation_span: Span,
}
```

#### 3. Compile-Time Execution
- Macros must be evaluated during compilation
- Requires interpreter running at compile-time
- Could be slow for complex macros

**Solution:**
- Optimize macro evaluator
- Cache macro expansions
- Limit macro complexity (no I/O, timeouts)

#### 4. Order of Definition
- Macros must be defined before use
- May require multiple compilation passes

**Solution:** Two-pass compilation
```rust
// Pass 1: Collect macro definitions
// Pass 2: Expand and compile
```

### Costs

❌ **Increased compile complexity** - Need macro expander, compile-time evaluator

❌ **Steeper learning curve** - Users must understand metaprogramming

❌ **Potential for abuse** - Macros can make code hard to read

❌ **Tooling complexity** - IDEs need to understand macro expansion

## Implementation Roadmap

### Phase 1: Foundation (Weeks 1-2)
- [ ] Add Quote/Unquote to AST
- [ ] Parse quote/unquote expressions
- [ ] Add Object::Ast for AST as runtime value
- [ ] Implement AST ↔ Object conversions

### Phase 2: Basic Macros (Weeks 3-4)
- [ ] Parse macro definitions
- [ ] Store macros in compiler environment
- [ ] Implement basic macro expansion (no nested macros)
- [ ] Test with simple examples (unless, when)

### Phase 3: Full Expansion (Weeks 5-6)
- [ ] Recursive macro expansion
- [ ] Handle nested macro calls
- [ ] Two-pass compilation (collect then expand)
- [ ] Integration tests

### Phase 4: Hygiene (Weeks 7-8)
- [ ] Implement gensym for unique variables
- [ ] Prevent accidental variable capture
- [ ] Preserve source locations through expansion

### Phase 5: Stdlib Migration (Weeks 9-10)
- [ ] Move type checking to macros (is_int, is_array, etc.)
- [ ] Move control flow to macros (unless, until)
- [ ] Create stdlib/macros.flx with common macros
- [ ] Update documentation

### Phase 6: Tooling (Weeks 11-12)
- [ ] Add --expand-macros flag to show expansions
- [ ] Improve error messages with macro context
- [ ] Source maps for debugging
- [ ] Documentation and examples

**Total estimated time:** 10-12 weeks (2.5-3 months)

## Success Metrics

### Code Quality
- **Builtin count:** 35 → 10 (71% reduction)
- **Compiler lines of code:** Increase ~500 lines (macro system), but isolated
- **Language flexibility:** Users can add features without compiler changes

### Performance
- **Macro expansion time:** < 10% of total compile time
- **Runtime performance:** No change (macros expand before execution)

### Developer Experience
- **Time to add "language feature":** Hours (write macro) vs. Days (modify compiler)
- **Community contributions:** Users can share macro libraries

## Alternatives Considered

### Alternative 1: No Macros (Status Quo)
**Pros:** Simpler compiler, easier to understand
**Cons:** Every feature requires compiler change, 35 hard-coded builtins

### Alternative 2: Preprocessor (C-style)
```flux
#define unless(cond, body) if (!(cond)) { body }
```
**Pros:** Very simple to implement
**Cons:** Text-based, not AST-aware, no hygiene, poor error messages

### Alternative 3: Compiler Plugins (Rust-style)
```flux
#[plugin]
fn my_macro(ast: TokenStream) -> TokenStream { ... }
```
**Pros:** Maximum power, can use full Rust
**Cons:** Requires proc-macro infrastructure, compilation complexity

**Recommendation:** Full macro system (like Elixir) - best balance of power and simplicity

## References

- [Writing an Interpreter in Go - Lost Chapter (Macros)](https://interpreterbook.com/lost/)
- [Elixir Macro Guide](https://elixir-lang.org/getting-started/meta/macros.html)
- [Lisp Macros](http://www.gigamonkeys.com/book/macros-standard-control-constructs.html)
- [Rust Macros](https://doc.rust-lang.org/book/ch19-06-macros.html)
- [Racket Macro System](https://docs.racket-lang.org/guide/macros.html)

## Conclusion

A macro system would fundamentally reshape Flux's architecture, following Elixir's philosophy:

**"Put power in macros — keep the compiler small."**

This enables:
- 71% reduction in VM builtins (35 → 10)
- User-defined control flow and language features
- Faster language evolution
- Community-driven extensions

**Recommendation:** Proceed with implementation after Phase 1 (Module Split) is complete.

---

## Appendix A: Syntax Comparison

### Elixir
```elixir
defmacro unless(condition, do: body) do
  quote do
    if not(unquote(condition)), do: unquote(body)
  end
end
```

### Proposed Flux
```flux
macro unless(condition, body) {
    quote {
        if !(unquote(condition)) {
            unquote(body)
        }
    }
}
```

### Lisp
```lisp
(defmacro unless (condition &body body)
  `(if (not ,condition)
       (progn ,@body)))
```

**Flux's syntax** aims to feel natural within the existing language while clearly marking macro expansion points.

## Appendix B: Minimal Working Example

**Goal:** Implement `unless` macro and demonstrate it works

**Step 1: Define macro**
```flux
macro unless(condition, body) {
    quote {
        if !(unquote(condition)) {
            unquote(body)
        }
    }
}
```

**Step 2: Use macro**
```flux
let x = 5;
unless(x > 10) {
    print("x is not greater than 10");
}
```

**Step 3: Compiler expands**
```flux
let x = 5;
if !(x > 10) {
    print("x is not greater than 10");
}
```

**Step 4: Compiles to bytecode normally**
```
OpConstant 0      // 5
OpSetGlobal 0     // x
OpGetGlobal 0     // x
OpConstant 1      // 10
OpGreaterThan
OpBang            // !
OpJumpIfFalse 10
OpConstant 2      // "x is not greater than 10"
OpGetBuiltin 0    // print
OpCall 1
```

This demonstrates the full pipeline: macro definition → expansion → compilation → execution.
