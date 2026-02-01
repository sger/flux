# Flux Grammar Improvements Proposal

This document analyzes the current Flux grammar and proposes targeted improvements for consistency, expressiveness, and maintainability.

---

## Table of Contents

1. [Current Grammar Analysis](#current-grammar-analysis)
2. [Identified Issues](#identified-issues)
3. [Proposed Improvements](#proposed-improvements)
4. [Formal Grammar Specification](#formal-grammar-specification)

---

## Current Grammar Analysis

### Token Types (Current)

```
Literals:    Int, Float, String, Ident
Operators:   + - * / ! < > == != =
Delimiters:  ( ) { } [ ] , ; : . ->
Keywords:    let fun if else return true false module import as Some None match
Special:     #{ (interpolation start)
```

### Expression Types (Current)

| Expression | Example | Notes |
|------------|---------|-------|
| Identifier | `foo`, `Math.sqrt` | Qualified names via MemberAccess |
| Integer | `42` | i64 |
| Float | `3.14` | f64 |
| String | `"hello"` | With escape sequences |
| InterpolatedString | `"Hello #{name}"` | Nested expressions |
| Boolean | `true`, `false` | |
| Prefix | `!x`, `-x` | Only `!` and `-` |
| Infix | `a + b` | Operator as String |
| If | `if c { a } else { b }` | Expression, optional else |
| Function | `fun(x) { x + 1 }` | Anonymous function |
| Call | `f(a, b)` | |
| Array | `[1, 2, 3]` | |
| Index | `arr[0]` | |
| Hash | `{a: 1, b: 2}` | |
| MemberAccess | `obj.field` | |
| Match | `match x { ... }` | |
| None | `None` | |
| Some | `Some(x)` | |

### Statement Types (Current)

| Statement | Example |
|-----------|---------|
| Let | `let x = 1;` |
| Assign | `x = 2;` |
| Return | `return x;` |
| Function | `fun foo(x) { ... }` |
| Module | `module M { ... }` |
| Import | `import M as N` |
| Expression | `print(x);` |

### Precedence Levels (Current)

```rust
Lowest       // default
Equals       // ==, !=
LessGreater  // <, >
Sum          // +, -
Product      // *, /
Prefix       // -x, !x
Call         // f(x)
Index        // a[i], a.b
```

---

## Identified Issues

### Issue 1: Operators Stored as Strings

**Current:**
```rust
Infix {
    left: Box<Expression>,
    operator: String,  // ← String, not enum
    right: Box<Expression>,
    span: Span,
}
```

**Problems:**
- No compile-time validation of operator strings
- Pattern matching requires string comparisons
- Easy to introduce typos ("==" vs "= =")
- No exhaustiveness checking

**Proposed fix:** Use an `Operator` enum

---

### Issue 2: Limited Pattern Types

**Current patterns:**
- `Wildcard` (`_`)
- `Literal` (numbers, strings, bools)
- `Identifier` (binding)
- `None`
- `Some(pattern)`

**Missing patterns:**
- Array patterns: `[a, b, ...rest]`
- Hash patterns: `{ x, y }`
- Tuple patterns: `(a, b)`
- Or-patterns: `A | B`
- Guards: `x if x > 0`
- As-patterns: `list @ [_, _, ...]`

---

### Issue 3: No Else-If Chain

**Current:** `else if` parsed as `else { if ... }` (nested)

```flux
if a {
    1
} else {
    if b {     // Nested if, not else-if
        2
    } else {
        3
    }
}
```

**Proposal:** Support `else if` as syntactic sugar or explicit construct

---

### Issue 4: Identifier Representation

**Current:** `type Identifier = String`

**Issues:**
- No string interning (memory inefficiency)
- No distinction between local names and qualified paths
- Cannot easily track source location

---

### Issue 5: Missing Precedence Levels

**Current gaps:**
- No level for `<=`, `>=` (same as `<`, `>`)
- No level for `&&`, `||` (logical)
- No level for `|>` (pipe)
- No level for `..` (range)

---

### Issue 6: Hash Key Flexibility

**Current:** Any expression can be a hash key

```rust
Hash {
    pairs: Vec<(Expression, Expression)>,  // Any expr as key
    ...
}
```

**Issue:** Runtime will fail on non-hashable keys

**Options:**
1. Keep flexible, validate at runtime (current)
2. Restrict to `Identifier | String | Integer` at parse time
3. Add static analysis warning

---

### Issue 7: No Distinction Between Expressions and Statements

**Current:** Everything is statement-based, with implicit expression returns

**Observation:** This is actually fine for a functional language, but could document better

---

## Proposed Improvements

### Improvement 1: Operator Enum

Replace string operators with a typed enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    // Arithmetic
    Add,        // +
    Sub,        // -
    Mul,        // *
    Div,        // /
    Mod,        // %

    // Comparison
    Eq,         // ==
    NotEq,      // !=
    Lt,         // <
    Gt,         // >
    LtEq,       // <=
    GtEq,       // >=

    // Logical
    And,        // &&
    Or,         // ||

    // Special
    Pipe,       // |>
    Range,      // ..
    RangeIncl,  // ..=
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,        // -
    Not,        // !
}

// Updated expression
pub enum Expression {
    // ...
    Infix {
        left: Box<Expression>,
        operator: BinaryOp,  // ← Typed!
        right: Box<Expression>,
        span: Span,
    },
    Prefix {
        operator: UnaryOp,   // ← Typed!
        right: Box<Expression>,
        span: Span,
    },
    // ...
}
```

**Benefits:**
- Compile-time operator validation
- Exhaustive pattern matching
- Better IDE support
- Clearer semantics

---

### Improvement 2: Enhanced Pattern Type

```rust
#[derive(Debug, Clone)]
pub enum Pattern {
    // Existing
    Wildcard,                           // _
    Literal(Literal),                   // 42, "str", true
    Identifier(Identifier),             // x
    None,                               // None
    Some(Box<Pattern>),                 // Some(x)

    // New: Either type patterns
    Left(Box<Pattern>),                 // Left(e)
    Right(Box<Pattern>),                // Right(x)

    // New: Array patterns
    Array {
        elements: Vec<Pattern>,
        rest: Option<Box<Pattern>>,     // ...tail
    },

    // New: Hash patterns
    Hash {
        fields: Vec<(Identifier, Option<Pattern>)>,  // {x, y: pat}
        rest: bool,                     // { x, ... }
    },

    // New: Tuple patterns
    Tuple {
        elements: Vec<Pattern>,
        rest: Option<Box<Pattern>>,
    },

    // New: Or patterns
    Or(Vec<Pattern>),                   // A | B | C

    // New: As patterns
    As {
        pattern: Box<Pattern>,
        binding: Identifier,            // pat @ name
    },

    // New: Guard (associated with arm, not pattern itself)
    // Represented in MatchArm instead
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expression>,      // ← New: if condition
    pub body: Expression,
}
```

**Examples of new patterns:**

```flux
// Array patterns
match list {
    [] -> "empty";
    [x] -> "single: #{x}";
    [x, y] -> "pair";
    [head, ...tail] -> "list with #{len(tail)} more";
}

// Hash patterns
match user {
    { name, age } -> "#{name} is #{age}";
    { name, ... } -> "just name: #{name}";
}

// Or patterns
match day {
    "Sat" | "Sun" -> "weekend";
    _ -> "weekday";
}

// Guards
match n {
    x if x > 0 -> "positive";
    x if x < 0 -> "negative";
    0 -> "zero";
}

// As patterns
match list {
    all @ [_, _, ...] -> "list has 2+: #{all}";
    _ -> "too short";
}
```

---

### Improvement 3: Else-If Chain

**Option A: Keep as syntactic sugar (recommended)**

No AST change needed. Just improve formatter/error messages.

```flux
// These are equivalent:
if a { 1 } else if b { 2 } else { 3 }
if a { 1 } else { if b { 2 } else { 3 } }
```

**Option B: Explicit elif construct**

```rust
pub enum Expression {
    If {
        condition: Box<Expression>,
        consequence: Block,
        else_ifs: Vec<(Expression, Block)>,  // ← New
        alternative: Option<Block>,
        span: Span,
    },
    // ...
}
```

**Recommendation:** Option A (keep simple, document well)

---

### Improvement 4: Identifier Improvements

**Step 1: String interning (optional, performance)**

```rust
// Using a string interner crate
type Symbol = internment::Intern<String>;
type Identifier = Symbol;
```

**Step 2: Distinguish simple vs qualified names**

```rust
#[derive(Debug, Clone)]
pub enum Name {
    Simple(Identifier),                    // foo
    Qualified(Vec<Identifier>),            // Foo.Bar.baz
}

// In expressions
pub enum Expression {
    Identifier {
        name: Name,  // ← Can be simple or qualified
        span: Span,
    },
    // Remove MemberAccess for qualified names
    // Keep MemberAccess only for dynamic access: obj.field
}
```

**Benefits:**
- Clear distinction between static paths and dynamic access
- Better error messages
- Easier module resolution

---

### Improvement 5: Updated Precedence

```rust
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Precedence {
    Lowest,
    Pipe,           // |>  (new)
    LogicalOr,      // ||  (new)
    LogicalAnd,     // &&  (new)
    Equality,       // ==, !=
    Comparison,     // <, >, <=, >=
    Range,          // .., ..=  (new)
    Sum,            // +, -
    Product,        // *, /, %
    Prefix,         // -x, !x
    Call,           // f(x)
    Index,          // a[i], a.b
}

pub fn token_precedence(token_type: &TokenType) -> Precedence {
    match token_type {
        TokenType::Pipe => Precedence::Pipe,
        TokenType::Or => Precedence::LogicalOr,
        TokenType::And => Precedence::LogicalAnd,
        TokenType::Eq | TokenType::NotEq => Precedence::Equality,
        TokenType::Lt | TokenType::Gt |
        TokenType::LtEq | TokenType::GtEq => Precedence::Comparison,
        TokenType::DotDot | TokenType::DotDotEq => Precedence::Range,
        TokenType::Plus | TokenType::Minus => Precedence::Sum,
        TokenType::Asterisk | TokenType::Slash |
        TokenType::Percent => Precedence::Product,
        TokenType::LParen => Precedence::Call,
        TokenType::LBracket | TokenType::Dot => Precedence::Index,
        _ => Precedence::Lowest,
    }
}
```

**Precedence rationale:**

```flux
// Pipe is lowest (binds loosest)
a + b |> f       // (a + b) |> f

// Logical or below and
a || b && c      // a || (b && c)

// Comparison below arithmetic
a + b < c + d    // (a + b) < (c + d)

// Range between comparison and arithmetic
1..10            // range
1..(a + b)       // range with expression
```

---

### Improvement 6: New Token Types

```rust
define_tokens! {
    symbols {
        // Existing
        Plus     => "+",
        Minus    => "-",
        Asterisk => "*",
        Slash    => "/",
        Bang     => "!",
        Lt       => "<",
        Gt       => ">",
        Eq       => "==",
        NotEq    => "!=",
        Assign   => "=",

        // New operators
        LtEq     => "<=",
        GtEq     => ">=",
        And      => "&&",
        Or       => "||",
        Percent  => "%",
        Pipe     => "|>",
        DotDot   => "..",
        DotDotEq => "..=",

        // New delimiters
        Underscore => "_",
        At         => "@",
        Backslash  => "\\",
        FatArrow   => "=>",  // alternative to ->

        // Existing delimiters
        LParen    => "(",
        RParen    => ")",
        LBrace    => "{",
        RBrace    => "}",
        Comma     => ",",
        Semicolon => ";",
        LBracket  => "[",
        RBracket  => "]",
        Colon     => ":",
        Dot       => ".",
        Arrow     => "->",
        InterpolationStart => "#{",
        StringEnd => "STRING_END",
    }

    keywords {
        // Existing
        Let    => "let",
        Fun    => "fun",
        If     => "if",
        Else   => "else",
        Return => "return",
        True   => "true",
        False  => "false",
        Module => "module",
        Import => "import",
        As     => "as",
        Some   => "Some",
        None   => "None",
        Match  => "match",

        // New keywords
        Type   => "type",
        For    => "for",
        In     => "in",
        While  => "while",
        Loop   => "loop",
        Break  => "break",
        Continue => "continue",
        Pub    => "pub",
        Mut    => "mut",
        Left   => "Left",
        Right  => "Right",
        With   => "with",      // for effects
        Effect => "effect",
        Actor  => "actor",
        Spawn  => "spawn",
        Send   => "send",
        Receive => "receive",
    }
}
```

---

### Improvement 7: New Expression Types

```rust
pub enum Expression {
    // ... existing ...

    // Lambda shorthand
    Lambda {
        parameters: Vec<Identifier>,
        body: Box<Expression>,
        span: Span,
    },

    // Pipe expression (could also use Infix)
    Pipe {
        left: Box<Expression>,
        right: Box<Expression>,
        span: Span,
    },

    // Range expression
    Range {
        start: Option<Box<Expression>>,
        end: Option<Box<Expression>>,
        inclusive: bool,
        span: Span,
    },

    // Tuple expression
    Tuple {
        elements: Vec<Expression>,
        span: Span,
    },

    // List comprehension
    ListComprehension {
        body: Box<Expression>,
        generators: Vec<Generator>,
        span: Span,
    },

    // If-let expression
    IfLet {
        pattern: Pattern,
        value: Box<Expression>,
        consequence: Block,
        alternative: Option<Block>,
        span: Span,
    },

    // Either types
    Left {
        value: Box<Expression>,
        span: Span,
    },
    Right {
        value: Box<Expression>,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct Generator {
    pub pattern: Pattern,       // x or (a, b)
    pub iterable: Expression,   // in arr
    pub condition: Option<Expression>,  // if x > 0
}
```

---

### Improvement 8: New Statement Types

```rust
pub enum Statement {
    // ... existing ...

    // Type declaration
    Type {
        name: Identifier,
        type_params: Vec<Identifier>,  // <T, U>
        variants: Vec<TypeVariant>,
        span: Span,
    },

    // For loop
    For {
        pattern: Pattern,
        iterable: Expression,
        body: Block,
        span: Span,
    },

    // While loop
    While {
        condition: Expression,
        body: Block,
        span: Span,
    },

    // Infinite loop
    Loop {
        body: Block,
        span: Span,
    },

    // Break and continue
    Break {
        value: Option<Expression>,
        span: Span,
    },
    Continue {
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct TypeVariant {
    pub name: Identifier,
    pub fields: Vec<TypeField>,
}

#[derive(Debug, Clone)]
pub struct TypeField {
    pub name: Option<Identifier>,  // None for positional
    pub type_annotation: Option<TypeExpr>,
}
```

---

## Formal Grammar Specification

### Current Grammar (Simplified EBNF)

```ebnf
program     = statement* ;

statement   = let_stmt
            | return_stmt
            | function_stmt
            | assign_stmt
            | module_stmt
            | import_stmt
            | expr_stmt ;

let_stmt    = "let" IDENT "=" expression ";"? ;
return_stmt = "return" expression? ";"? ;
function_stmt = "fun" IDENT "(" params? ")" block ;
assign_stmt = IDENT "=" expression ";"? ;
module_stmt = "module" IDENT block ;
import_stmt = "import" qualified_name ("as" IDENT)? ";"? ;
expr_stmt   = expression ";"? ;

expression  = prefix_expr | infix_expr | primary ;

prefix_expr = ("!" | "-") expression ;

infix_expr  = expression operator expression ;

operator    = "+" | "-" | "*" | "/" | "==" | "!=" | "<" | ">" ;

primary     = IDENT
            | INT
            | FLOAT
            | STRING
            | "true" | "false"
            | "None"
            | "Some" "(" expression ")"
            | array
            | hash
            | if_expr
            | match_expr
            | function_lit
            | "(" expression ")"
            | primary "[" expression "]"     // index
            | primary "." IDENT              // member access
            | primary "(" arguments? ")"    // call
            ;

array       = "[" (expression ("," expression)*)? "]" ;
hash        = "{" (hash_pair ("," hash_pair)*)? "}" ;
hash_pair   = expression ":" expression ;

if_expr     = "if" expression block ("else" block)? ;
match_expr  = "match" expression "{" match_arm* "}" ;
match_arm   = pattern "->" expression ";"? ;

pattern     = "_"
            | IDENT
            | literal
            | "None"
            | "Some" "(" pattern ")" ;

function_lit = "fun" "(" params? ")" block ;
block       = "{" statement* "}" ;
params      = IDENT ("," IDENT)* ;
arguments   = expression ("," expression)* ;

qualified_name = IDENT ("." IDENT)* ;
```

### Proposed Grammar (Extended)

```ebnf
program     = statement* ;

(* ═══════════════════════════════════════════════════════════════════ *)
(*  STATEMENTS                                                          *)
(* ═══════════════════════════════════════════════════════════════════ *)

statement   = let_stmt
            | return_stmt
            | function_stmt
            | assign_stmt
            | module_stmt
            | import_stmt
            | type_stmt          (* NEW *)
            | for_stmt           (* NEW *)
            | while_stmt         (* NEW *)
            | loop_stmt          (* NEW *)
            | break_stmt         (* NEW *)
            | continue_stmt      (* NEW *)
            | expr_stmt ;

let_stmt    = "let" pattern "=" expression ";"? ;  (* pattern instead of IDENT *)

type_stmt   = "type" IDENT type_params? "{" type_variant* "}" ;
type_params = "<" IDENT ("," IDENT)* ">" ;
type_variant = IDENT ("(" type_fields ")")? ;
type_fields = type_field ("," type_field)* ;
type_field  = (IDENT ":")? type_expr ;

for_stmt    = "for" pattern "in" expression block ;
while_stmt  = "while" expression block ;
loop_stmt   = "loop" block ;
break_stmt  = "break" expression? ";"? ;
continue_stmt = "continue" ";"? ;

(* ═══════════════════════════════════════════════════════════════════ *)
(*  EXPRESSIONS (by precedence, lowest to highest)                      *)
(* ═══════════════════════════════════════════════════════════════════ *)

expression  = pipe_expr ;

pipe_expr   = or_expr ("|>" or_expr)* ;

or_expr     = and_expr ("||" and_expr)* ;

and_expr    = equality_expr ("&&" equality_expr)* ;

equality_expr = comparison_expr (("==" | "!=") comparison_expr)* ;

comparison_expr = range_expr (("<" | ">" | "<=" | ">=") range_expr)* ;

range_expr  = additive_expr ((".." | "..=") additive_expr)? ;

additive_expr = multiplicative_expr (("+" | "-") multiplicative_expr)* ;

multiplicative_expr = prefix_expr (("*" | "/" | "%") prefix_expr)* ;

prefix_expr = ("!" | "-")* postfix_expr ;

postfix_expr = primary (
                 "(" arguments? ")"          (* call *)
               | "[" expression "]"          (* index *)
               | "." IDENT                   (* member *)
               )* ;

primary     = IDENT
            | INT
            | FLOAT
            | STRING
            | interpolated_string
            | "true" | "false"
            | "None"
            | "Some" "(" expression ")"
            | "Left" "(" expression ")"      (* NEW *)
            | "Right" "(" expression ")"     (* NEW *)
            | array
            | tuple                          (* NEW *)
            | hash
            | if_expr
            | if_let_expr                    (* NEW *)
            | match_expr
            | function_lit
            | lambda                         (* NEW *)
            | list_comprehension             (* NEW *)
            | "(" expression ")" ;

(* ═══════════════════════════════════════════════════════════════════ *)
(*  NEW EXPRESSION TYPES                                                *)
(* ═══════════════════════════════════════════════════════════════════ *)

lambda      = "\\" params "->" expression
            | "\\" params "->" block ;

tuple       = "(" ")"                        (* unit *)
            | "(" expression "," ")"         (* single-element *)
            | "(" expression ("," expression)+ ")" ;

if_let_expr = "if" "let" pattern "=" expression block ("else" block)? ;

list_comprehension = "[" expression "for" generators "]" ;
generators  = generator ("for" generator | "if" expression)* ;
generator   = pattern "in" expression ;

(* ═══════════════════════════════════════════════════════════════════ *)
(*  PATTERNS (enhanced)                                                 *)
(* ═══════════════════════════════════════════════════════════════════ *)

pattern     = or_pattern ;

or_pattern  = as_pattern ("|" as_pattern)* ;

as_pattern  = primary_pattern ("@" IDENT)? ;

primary_pattern = "_"                        (* wildcard *)
            | IDENT                          (* binding *)
            | literal                        (* literal match *)
            | "None"
            | "Some" "(" pattern ")"
            | "Left" "(" pattern ")"         (* NEW *)
            | "Right" "(" pattern ")"        (* NEW *)
            | UPPER_IDENT "(" pattern* ")"   (* constructor *)
            | array_pattern                  (* NEW *)
            | tuple_pattern                  (* NEW *)
            | hash_pattern                   (* NEW *)
            | "(" pattern ")" ;

array_pattern = "[" "]"
              | "[" pattern ("," pattern)* ("," "..." IDENT?)? "]" ;

tuple_pattern = "(" ")"
              | "(" pattern "," ")"
              | "(" pattern ("," pattern)+ ")" ;

hash_pattern = "{" "}"
             | "{" hash_field_pattern ("," hash_field_pattern)* ("," "...")? "}" ;
hash_field_pattern = IDENT (":" pattern)? ;

(* ═══════════════════════════════════════════════════════════════════ *)
(*  MATCH ARMS (with guards)                                            *)
(* ═══════════════════════════════════════════════════════════════════ *)

match_arm   = pattern guard? "->" expression ";"? ;
guard       = "if" expression ;

(* ═══════════════════════════════════════════════════════════════════ *)
(*  EXISTING (unchanged)                                                *)
(* ═══════════════════════════════════════════════════════════════════ *)

array       = "[" (expression ("," expression)*)? "]" ;
hash        = "{" (hash_pair ("," hash_pair)*)? "}" ;
hash_pair   = expression ":" expression ;
if_expr     = "if" expression block ("else" (if_expr | block))? ;
match_expr  = "match" expression "{" match_arm* "}" ;
function_lit = "fun" "(" params? ")" block ;
block       = "{" statement* "}" ;
params      = IDENT ("," IDENT)* ;
arguments   = expression ("," expression)* ;
```

---

## Summary of Changes

### Tokens to Add
| Token | Symbol | Purpose |
|-------|--------|---------|
| `LtEq` | `<=` | Less than or equal |
| `GtEq` | `>=` | Greater than or equal |
| `And` | `&&` | Logical AND |
| `Or` | `\|\|` | Logical OR |
| `Percent` | `%` | Modulo |
| `Pipe` | `\|>` | Pipe operator |
| `DotDot` | `..` | Range (exclusive) |
| `DotDotEq` | `..=` | Range (inclusive) |
| `Backslash` | `\` | Lambda start |
| `At` | `@` | As-pattern |

### Keywords to Add
| Keyword | Purpose |
|---------|---------|
| `type` | ADT declaration |
| `for` | For loop |
| `in` | For loop / generators |
| `while` | While loop |
| `loop` | Infinite loop |
| `break` | Loop exit |
| `continue` | Loop continue |
| `Left` | Either left constructor |
| `Right` | Either right constructor |

### AST Changes
1. Replace `operator: String` with `operator: BinaryOp/UnaryOp`
2. Extend `Pattern` enum with array, tuple, hash, or, as patterns
3. Add `guard: Option<Expression>` to `MatchArm`
4. Add `Lambda` expression type
5. Add `Tuple` expression type
6. Add `Range` expression type
7. Add `ListComprehension` expression type
8. Add `IfLet` expression type
9. Add `Type`, `For`, `While`, `Loop`, `Break`, `Continue` statements

### Precedence Additions
Add levels for: `Pipe`, `LogicalOr`, `LogicalAnd`, `Range`

---

## Migration Path

### Phase 1: Non-Breaking Additions
1. Add new tokens (<=, >=, &&, ||, %, |>)
2. Add new precedence levels
3. Keep operator as String (for now)

### Phase 2: AST Improvements
1. Replace operator String with enum
2. Update all compiler phases
3. Add new pattern types

### Phase 3: New Constructs
1. Add type declarations
2. Add loop constructs
3. Add lambda shorthand
4. Add list comprehensions

This approach allows incremental adoption without breaking existing code.
