# Proposal 027: Flux Language Syntax Specification

**Status:** Proposed
**Priority:** High
**Created:** 2026-02-12
**Related:** Proposal 025 (Pure FP Vision), Proposal 026 (Concurrency Model), Proposal 004 (Language Features)

## Overview

This document defines the complete syntax for Flux as a pure functional language. It covers current syntax (what exists today), confirmed additions, and the target syntax for the full language. Every construct includes grammar rules and examples.

The syntax philosophy: **Rust's structure, JS's familiarity, FP's power.**

---

## 1. Lexical Elements

### 1.1 Keywords

**Current (13):**
```
let  fun  if  else  return  true  false
match  module  import  as  Some  None  Left  Right
```

**New keywords (phased):**

| Phase | Keywords | Purpose |
|-------|----------|---------|
| Types | `type`, `trait`, `impl`, `deriving`, `where` | Type system |
| Effects | `with`, `effect`, `handle`, `resume`, `perform` | Effect system |
| Concurrency | `async`, `await`, `actor`, `spawn`, `send`, `receive`, `reply`, `stop` | Async + Actors |
| Patterns | `if let`, `else` (in let-else) | Enhanced pattern matching |
| Reserved | `do`, `yield`, `lazy`, `mut`, `pub`, `self`, `Self`, `for`, `in`, `try` | Future use |

### 1.2 Operators

**Current:**
```
+   -   *   /   %           Arithmetic
==  !=  <   >   <=  >=      Comparison
&&  ||  !                   Logical
|>                          Pipe
->                          Arrow (lambdas, match arms)
=                           Binding
.                           Member access
```

**New operators:**

| Operator | Name | Precedence | Purpose |
|----------|------|------------|---------|
| `>>` | Compose | 2 (above pipe) | Function composition: `(f >> g)(x) = g(f(x))` |
| `<<` | ComposeBack | 2 | Reverse composition: `(f << g)(x) = f(g(x))` |
| `?` | Try | 15 (postfix) | Error propagation: `expr?` unwraps Ok or returns Err |
| `..` | Range | 8 | Exclusive range: `1..10` |
| `..=` | RangeInclusive | 8 | Inclusive range: `1..=10` |
| `...` | Spread | — | Spread/rest: `{ ...record }`, `[head, ...tail]` |
| `=>` | FatArrow | — | Match arms (replaces `->` in match context) |
| `++` | Concat | 6 | String/array concatenation |

### 1.3 Precedence Table (Low to High)

| Level | Operators | Associativity |
|-------|-----------|---------------|
| 1 | `\|>` | Left |
| 2 | `>>` `<<` | Left |
| 3 | `\|\|` | Left |
| 4 | `&&` | Left |
| 5 | `==` `!=` | Left |
| 6 | `<` `>` `<=` `>=` | Left |
| 7 | `++` | Left |
| 8 | `..` `..=` | None |
| 9 | `+` `-` | Left |
| 10 | `*` `/` `%` | Left |
| 11 | `-` `!` (prefix) | Right |
| 12 | `?` (postfix) | Left |
| 13 | `.` `[]` `()` | Left |

### 1.4 Comments

```flux
// Single-line comment

/* Block comment */

/* Nested /* block */ comments */

/// Documentation comment (attached to next item)

//! Module-level documentation comment
```

### 1.5 Literals

```flux
// Integers
42
-17
0
1_000_000           // underscores for readability (new)

// Floats
3.14
-0.5
1.0e10              // scientific notation (new)
1_000.50

// Booleans
true
false

// Strings
"hello"
"line 1\nline 2"    // escape sequences: \n \t \\ \" \#{
"sum is #{1 + 2}"   // string interpolation

// None
None

// Characters (future)
'a'
'\n'
```

---

## 2. Bindings and Variables

### 2.1 Let Bindings

All bindings are immutable. There is no `var`, `mut`, or reassignment.

```flux
// Simple binding
let x = 42
let name = "Flux"

// Type-annotated (with type system)
let x: Int = 42
let name: String = "Flux"

// Pattern binding
let (a, b) = (1, 2)
let [first, ...rest] = [1, 2, 3]
let { name, age } = user

// With type annotation
let (a, b): (Int, Int) = (1, 2)
```

**Grammar:**
```
let_stmt := 'let' pattern (':' type)? '=' expression
```

### 2.2 Let-Else (Early Return)

Bind a pattern or take an alternative path:

```flux
let Some(value) = find_user(id) else {
  return None
}
// value is bound here
process(value)
```

**Grammar:**
```
let_else_stmt := 'let' pattern '=' expression 'else' block
```

---

## 3. Functions

### 3.1 Named Functions

```flux
// Basic function
fun add(x, y) {
  x + y
}

// With type annotations
fun add(x: Int, y: Int): Int {
  x + y
}

// With effect annotation
fun greet(name: String) with IO {
  print("Hello #{name}!")
}

// Single-expression body (no braces needed)
fun double(x) = x * 2

// With default parameters
fun greet(name, greeting = "Hello") {
  "#{greeting}, #{name}!"
}

// With type parameters
fun identity<T>(x: T): T = x

fun map<A, B>(list: List<A>, f: (A) -> B): List<B> {
  match list {
    Nil => Nil,
    Cons(head, tail) => Cons(f(head), map(tail, f)),
  }
}
```

**Grammar:**
```
fun_stmt := 'fun' IDENT type_params? '(' params ')' (':' type)? effect_clause? ('=' expression | block)
params := (param (',' param)* ','?)?
param := pattern (':' type)? ('=' expression)?
type_params := '<' IDENT (',' IDENT)* '>'
effect_clause := 'with' effect (',' effect)*
```

### 3.2 Lambda Expressions

```flux
// Backslash-arrow syntax (primary)
\x -> x * 2
\(x, y) -> x + y
\() -> 42

// Block body
\x -> {
  let y = x * 2
  y + 1
}

// With type annotations
\(x: Int, y: Int) -> x + y

// Anonymous fun (alternative, for longer bodies)
fun(x, y) { x + y }
```

**Grammar:**
```
lambda := '\' lambda_params '->' expression
lambda_params := IDENT | '(' params ')'
```

### 3.3 Function Calls

```flux
// Standard call
add(1, 2)

// Method-style via pipe
1 |> add(2)              // => add(1, 2)

// Chained pipes
[1, 2, 3]
  |> filter(\x -> x > 1)
  |> map(\x -> x * 2)
  |> fold(0, \a, b -> a + b)

// Module-qualified call
Math.double(5)

// Call with trailing lambda (future consideration)
list |> map \x -> x * 2
```

### 3.4 Partial Application and Currying

```flux
// All functions are curried — supplying fewer args returns a new function
let inc = add(1)         // (Int) -> Int
let result = inc(5)      // 6

// Placeholder syntax for non-first-arg partial application
let half = div(_, 2)     // (Int) -> Int
let is_positive = gt(_, 0)

// Works naturally with pipes
[1, 2, 3]
  |> map(add(10))        // [11, 12, 13]
  |> filter(gt(_, 5))    // [11, 12, 13]
```

### 3.5 Function Composition

```flux
// Forward composition: (f >> g)(x) = g(f(x))
let process = parse >> validate >> transform
let result = process(input)

// Backward composition: (f << g)(x) = f(g(x))
let check = is_valid << parse
```

---

## 4. Control Flow

### 4.1 If Expression

Everything is an expression — `if` returns a value.

```flux
// Basic
if condition {
  value_a
} else {
  value_b
}

// Chained (else-if)
if x > 0 {
  "positive"
} else if x < 0 {
  "negative"
} else {
  "zero"
}

// As expression in binding
let label = if score >= 90 { "A" } else if score >= 80 { "B" } else { "C" }
```

**Grammar:**
```
if_expr := 'if' expression block ('else' (if_expr | block))?
```

### 4.2 If-Let

Pattern matching in conditionals:

```flux
if let Some(user) = find_user(id) {
  greet(user)
} else {
  "not found"
}

// Chained
if let Some(user) = find_user(id) {
  if let Some(email) = user.email {
    send_email(email)
  }
}
```

**Grammar:**
```
if_let_expr := 'if' 'let' pattern '=' expression block ('else' block)?
```

### 4.3 Match Expression

```flux
// Basic match
match value {
  0 => "zero",
  1 => "one",
  n => "other: #{n}",
}

// With guards
match score {
  n if n >= 90 => "A",
  n if n >= 80 => "B",
  n if n >= 70 => "C",
  _ => "F",
}

// Nested patterns
match result {
  Ok(Some(value)) => process(value),
  Ok(None) => "empty",
  Err(msg) => "error: #{msg}",
}

// Or-patterns
match day {
  "Saturday" | "Sunday" => "weekend",
  _ => "weekday",
}
```

**Note on `=>` vs `->`:** Match arms use `=>` (fat arrow) to distinguish from lambda arrows and type arrows. This is consistent with Rust/Scala.

**Grammar:**
```
match_expr := 'match' expression '{' match_arm (',' match_arm)* ','? '}'
match_arm := pattern ('|' pattern)* ('if' expression)? '=>' expression
```

---

## 5. Pattern Matching

### 5.1 Pattern Types

```flux
// Wildcard — matches anything, binds nothing
_

// Literal — exact value match
42
"hello"
true

// Identifier — matches anything, binds to name
x
name

// Constructor patterns
Some(inner)
None
Ok(value)
Err(message)
Circle(radius)

// Tuple pattern
(a, b)
(x, y, z)

// Array pattern
[]                     // empty
[single]               // exactly one element
[first, second]        // exactly two
[head, ...tail]        // head + rest (at least one)
[a, b, ...rest]        // first two + rest
[_, _, third, ...]     // skip first two, bind third

// Record pattern
{ name, age }                // shorthand (bind name, age)
{ name: n, age: a }         // rename bindings
{ name, ...rest }           // rest of fields
{ active: true, name }      // literal field + binding

// Nested patterns
Some([first, ...rest])
Ok({ name, age })
(Some(x), None)

// Or-pattern
"yes" | "y" | "Y"
Some(0) | None
```

**Grammar:**
```
pattern := '_'
         | literal
         | IDENT
         | UPPER_IDENT '(' pattern (',' pattern)* ')'
         | '(' pattern (',' pattern)* ')'
         | '[' array_pattern ']'
         | '{' record_pattern '}'
         | pattern '|' pattern

array_pattern := (pattern (',' pattern)* (',' '...' IDENT?)? ','?)?
record_pattern := (field_pattern (',' field_pattern)* (',' '...' IDENT?)? ','?)?
field_pattern := IDENT (':' pattern)?
```

### 5.2 Exhaustiveness

The compiler checks that match expressions cover all cases:

```flux
type Color = Red | Green | Blue

match color {
  Red => "red",
  Green => "green",
  // Compile error: non-exhaustive match — missing Blue
}
```

---

## 6. Type System

### 6.1 Built-in Types

```
Int                     64-bit signed integer
Float                   64-bit float
Bool                    Boolean
String                  UTF-8 string
None                    Absence type
```

### 6.2 Type Constructors

```flux
Option<T>              = Some(T) | None
Result<T, E>           = Ok(T) | Err(E)
List<T>                = Cons(T, List<T>) | Nil
Array<T>               Fixed-size ordered collection
Hash<K, V>             Key-value map
(A, B)                 Tuple
(A) -> B               Function type
(A) -> B with E        Effectful function type
Future<T>              Async result
ActorRef<M>            Actor handle (M = message type)
```

### 6.3 Algebraic Data Types

```flux
// Sum type (tagged union)
type Shape
  = Circle(Float)
  | Rectangle(Float, Float)
  | Triangle(Float, Float, Float)

// With type parameters
type Tree<T>
  = Leaf(T)
  | Node(Tree<T>, T, Tree<T>)

// Single-line form
type Ordering = Less | Equal | Greater

// Recursive
type List<T> = Cons(T, List<T>) | Nil
```

**Grammar:**
```
type_decl := 'type' UPPER_IDENT type_params? '=' variant ('|' variant)*
variant := UPPER_IDENT ('(' type (',' type)* ')')?
```

### 6.4 Record Types

```flux
// Named record
type User = {
  name: String,
  age: Int,
  active: Bool,
}

// With defaults
type Config = {
  host: String,
  port: Int = 8080,
  debug: Bool = false,
}

// Generic record
type Pair<A, B> = {
  first: A,
  second: B,
}
```

**Grammar:**
```
record_decl := 'type' UPPER_IDENT type_params? '=' '{' field_decl (',' field_decl)* ','? '}'
field_decl := IDENT ':' type ('=' expression)?
```

### 6.5 Type Aliases

```flux
type UserId = Int
type Predicate<T> = (T) -> Bool
type Handler<T> = (T) -> Result<(), String> with IO
```

### 6.6 Traits

```flux
// Trait declaration
trait Show {
  fun show(self): String
}

trait Eq {
  fun eq(self, other: Self): Bool
  fun neq(self, other: Self): Bool = !self.eq(other)   // default impl
}

trait Ord: Eq {         // trait inheritance
  fun compare(self, other: Self): Ordering
}

trait Functor<F> {
  fun map<A, B>(self: F<A>, f: (A) -> B): F<B>
}

// Implementation
impl Show for User {
  fun show(self) = "User(#{self.name}, #{self.age})"
}

impl Show for Shape {
  fun show(self) = match self {
    Circle(r) => "Circle(#{r})",
    Rectangle(w, h) => "Rectangle(#{w}, #{h})",
    Triangle(a, b, c) => "Triangle(#{a}, #{b}, #{c})",
  }
}

// Derived traits
type Point = { x: Float, y: Float } deriving (Show, Eq)

// Trait bounds
fun print_all<T: Show>(items: List<T>) with IO {
  items |> each(\item -> print(show(item)))
}

fun sort<T: Ord>(list: List<T>): List<T> {
  // ...
}
```

**Grammar:**
```
trait_decl := 'trait' UPPER_IDENT type_params? (':' trait_bound (',' trait_bound)*)? '{' trait_method* '}'
trait_method := 'fun' IDENT '(' params ')' (':' type)? ('=' expression)?
impl_decl := 'impl' UPPER_IDENT 'for' type '{' fun_stmt* '}'
```

### 6.7 Type Annotations

Annotations are always optional — the compiler infers types.

```flux
// Variable
let x: Int = 42

// Function
fun add(x: Int, y: Int): Int = x + y

// Lambda
let double: (Int) -> Int = \x -> x * 2

// Complex types
let transform: (List<Int>) -> List<String> with IO = \xs -> {
  xs |> map(\x -> {
    print("processing #{x}")
    to_string(x)
  })
}
```

---

## 7. Effects

### 7.1 Effect Declarations

```flux
// Built-in effects (provided by runtime)
effect IO              // Console, file, network
effect Async           // Asynchronous operations
effect Fail<E>         // Recoverable errors

// User-defined effects (future)
effect State<S> {
  fun get(): S
  fun set(s: S): ()
}

effect Random {
  fun random_int(min: Int, max: Int): Int
  fun random_float(): Float
}
```

### 7.2 Effect Annotations

```flux
// Pure function (default, no annotation needed)
fun add(x, y) = x + y

// Effectful function
fun greet(name) with IO {
  print("Hello #{name}!")
}

// Multiple effects
fun fetch(url) with IO, Async, Fail<HttpError> {
  let response = await http_get(url)
  if response.status != 200 {
    fail(HttpError(response.status, response.body))
  }
  parse_json(response.body)?
}

// Effects are inferred — annotation is optional documentation
fun process(data) {        // compiler infers: with IO
  print("Processing...")
  transform(data)
}
```

### 7.3 Effect Handlers

```flux
// Handle an effect
let result = handle {
  fetch_data("http://example.com")
} with {
  IO.http_get(url) => resume({ status: 200, body: "mock" }),
}

// Useful for testing
fun test_parser() {
  let result = handle {
    parse_config()
  } with {
    IO.read_file(_) => resume("test=true\nvalue=42"),
  }
  assert(result.test == true)
}
```

### 7.4 Error Propagation

```flux
// The ? operator: unwrap Ok or early-return Err
fun load_config(path) with IO, Fail<ConfigError> {
  let content = read_file(path)?
  let parsed = parse_toml(content)?
  validate(parsed)?
}

// Try block — catch Fail effect
let result = try {
  load_config("app.toml")
}
// result: Result<Config, ConfigError>
```

---

## 8. Data Structures

### 8.1 Arrays

```flux
// Literal
let numbers = [1, 2, 3, 4, 5]
let empty: Array<Int> = []

// Operations (all return new arrays)
let doubled = numbers |> map(\x -> x * 2)
let evens = numbers |> filter(\x -> x % 2 == 0)
let sum = numbers |> fold(0, \a, b -> a + b)

// Indexing (returns Option)
let first = numbers[0]          // Some(1)
let oob = numbers[99]           // None

// Slicing (new)
let middle = numbers[1..3]      // [2, 3]
let tail = numbers[1..]         // [2, 3, 4, 5]
let head = numbers[..3]         // [1, 2, 3]

// Spread
let extended = [...numbers, 6, 7]
let joined = [...a, ...b]
```

### 8.2 Tuples

```flux
// Literal
let pair = (1, "hello")
let triple = (1, 2.0, true)
let unit = ()

// Access
let x = pair.0                  // 1
let y = pair.1                  // "hello"

// Destructuring
let (a, b) = pair
let (x, _, z) = triple          // ignore second element
```

### 8.3 Records

```flux
// Construction
let user = User { name: "Alice", age: 30, active: true }

// Shorthand (when variable name matches field)
let name = "Alice"
let age = 30
let user = User { name, age, active: true }

// Access
let n = user.name

// Functional update (spread)
let older = { ...user, age: user.age + 1 }

// Anonymous records (structural, no type name needed)
let point = { x: 10, y: 20 }
```

### 8.4 Hash Maps

```flux
// Literal
let scores = { "alice": 95, "bob": 87 }
let empty: Hash<String, Int> = {}

// Access (returns Option)
let alice = scores["alice"]     // Some(95)
let missing = scores["unknown"] // None

// Operations
let updated = merge(scores, { "carol": 92 })
let without = delete(scores, "bob")
let ks = keys(scores)           // ["alice", "bob"]
```

### 8.5 Ranges

```flux
// Exclusive
1..10                           // 1, 2, ..., 9

// Inclusive
1..=10                          // 1, 2, ..., 10

// Open ranges (context-dependent)
..10                            // 0, 1, ..., 9
5..                             // 5, 6, 7, ... (lazy)

// Use with operations
1..=100 |> filter(\x -> x % 2 == 0) |> take(10)

// Array slicing
let middle = arr[2..5]
```

---

## 9. Modules

### 9.1 Module Declaration

```flux
//! Module documentation

module Modules.Math {
  /// Doubles a number
  fun double(x) = x * 2

  /// Squares a number
  fun square(x) = x * x

  // Private (underscore prefix convention)
  fun _helper(x) = x + 1
}
```

### 9.2 Imports

```flux
// Full module import
import Modules.Math

// With alias
import Modules.Math as M

// Usage
Modules.Math.double(5)
M.double(5)
```

### 9.3 Module Rules

- Module names are PascalCase, dot-separated: `Modules.Data.Users`
- Imports are top-level only (not inside functions)
- Functions starting with `_` are private
- Cycles are detected at compile time (error E035)
- Forward references within a module are allowed

---

## 10. Concurrency

### 10.1 Async/Await

```flux
// Async function
fun fetch_user(id) with IO, Async {
  let response = await http_get("/users/#{id}")
  parse_json(response.body)
}

// Spawn concurrent task
let future = async fetch_user(42)

// Await result
let user = await future

// Parallel execution
fun load_all(ids) with IO, Async {
  let futures = ids |> map(\id -> async fetch_user(id))
  futures |> map(\f -> await f)
}

// Sleep / timers
await sleep(1000)
```

### 10.2 Actors

```flux
// Actor definition
actor Counter(initial: Int) {
  state count = initial

  receive Increment {
    count = count + 1
  }

  receive Get {
    reply(count)
  }

  receive Add(n: Int) {
    count = count + n
    reply(count)
  }
}

// Spawn
let counter = spawn Counter(0)

// Fire-and-forget
send(counter, Increment)

// Request-reply
let value = await ask(counter, Get)

// Shutdown
stop(counter)
```

### 10.3 Actor Monitoring

```flux
// Watch for actor failure
let ref = monitor(worker)

// Handle in receive
receive ActorDown(ref, reason) {
  let replacement = spawn Worker()
  monitor(replacement)
}
```

---

## 11. Comprehensions

### 11.1 List Comprehensions

```flux
// Basic
[x * 2 for x in numbers]

// With filter
[x for x in numbers if x > 0]

// Multiple generators
[(x, y) for x in xs for y in ys]

// With pattern matching
[name for { name, active: true } in users]

// Nested
[cell for row in matrix for cell in row]
```

### 11.2 Hash Comprehensions

```flux
{ user.id: user for user in users }
{ k: v * 2 for (k, v) in entries if v > 0 }
```

**Grammar:**
```
list_comp := '[' expression 'for' pattern 'in' expression ('if' expression)? ('for' pattern 'in' expression ('if' expression)?)* ']'
hash_comp := '{' expression ':' expression 'for' pattern 'in' expression ('if' expression)? '}'
```

---

## 12. Where Clauses

```flux
fun bmi_category(weight, height) {
  classify(index)
  where index = weight / (height * height)
  where classify = \i -> match i {
    i if i < 18.5 => "underweight",
    i if i < 25.0 => "normal",
    _ => "overweight",
  }
}

fun distance(p1, p2) =
  sqrt(dx * dx + dy * dy)
  where dx = p2.x - p1.x
  where dy = p2.y - p1.y
```

**Grammar:**
```
where_clause := expression ('where' IDENT '=' expression)*
```

---

## 13. Complete Grammar Summary

```ebnf
(* === Top Level === *)
program         := statement*
statement       := let_stmt | fun_stmt | type_decl | trait_decl | impl_decl
                 | actor_decl | module_stmt | import_stmt | expr_stmt

(* === Statements === *)
let_stmt        := 'let' pattern (':' type)? '=' expression
                 | 'let' pattern '=' expression 'else' block
fun_stmt        := 'fun' IDENT type_params? '(' params ')' (':' type)? effect_clause? ('=' expression | block)
module_stmt     := 'module' module_path '{' statement* '}'
import_stmt     := 'import' module_path ('as' IDENT)?
expr_stmt       := expression

(* === Expressions === *)
expression      := if_expr | match_expr | lambda | let_in | try_expr | handle_expr
                 | infix_expr | prefix_expr | postfix_expr | call_expr
                 | index_expr | member_expr | primary

if_expr         := 'if' expression block ('else' (if_expr | block))?
                 | 'if' 'let' pattern '=' expression block ('else' block)?
match_expr      := 'match' expression '{' match_arm (',' match_arm)* ','? '}'
match_arm       := pattern ('|' pattern)* ('if' expression)? '=>' expression
lambda          := '\' lambda_params '->' expression
                 | 'fun' '(' params ')' block
let_in          := expression where_clause*
try_expr        := 'try' block
handle_expr     := 'handle' block 'with' '{' handler_arm* '}'

infix_expr      := expression operator expression
prefix_expr     := ('!' | '-') expression
postfix_expr    := expression '?'
call_expr       := expression '(' args ')'
index_expr      := expression '[' expression ']'
member_expr     := expression '.' IDENT

primary         := IDENT | literal | array | hash | tuple | list_comp | hash_comp
                 | '(' expression ')' | 'async' expression
                 | 'await' expression | 'spawn' IDENT '(' args ')'
                 | 'send' '(' expression ',' expression ')'

(* === Patterns === *)
pattern         := '_'
                 | literal
                 | IDENT
                 | UPPER_IDENT ('(' pattern (',' pattern)* ')')?
                 | '(' pattern (',' pattern)* ')'
                 | '[' (pattern (',' pattern)* (',' '...' IDENT?)?)? ']'
                 | '{' (field_pattern (',' field_pattern)* (',' '...' IDENT?)?)? '}'
                 | pattern '|' pattern

(* === Types === *)
type            := IDENT | UPPER_IDENT type_args?
                 | '(' type (',' type)* ')' | '(' type ')' '->' type
                 | '{' field_type (',' field_type)* '}' | type 'with' effect+
type_args       := '<' type (',' type)* '>'
type_params     := '<' IDENT (',' IDENT)* '>'
field_type      := IDENT ':' type

(* === Types System === *)
type_decl       := 'type' UPPER_IDENT type_params? '=' type_body ('deriving' '(' trait_list ')')?
type_body       := variant ('|' variant)*
                 | '{' field_decl (',' field_decl)* ','? '}'
                 | type
variant         := UPPER_IDENT ('(' type (',' type)* ')')?
field_decl      := IDENT ':' type ('=' expression)?

trait_decl      := 'trait' UPPER_IDENT type_params? (':' UPPER_IDENT (',' UPPER_IDENT)*)? '{' trait_method* '}'
trait_method     := 'fun' IDENT '(' params ')' (':' type)? ('=' expression)?
impl_decl       := 'impl' UPPER_IDENT type_args? 'for' type '{' fun_stmt* '}'

(* === Effects === *)
effect_clause   := 'with' UPPER_IDENT (',' UPPER_IDENT)*
effect_decl     := 'effect' UPPER_IDENT type_params? ('{' effect_method* '}')?
effect_method   := 'fun' IDENT '(' params ')' ':' type
handler_arm     := UPPER_IDENT '.' IDENT '(' params ')' '=>' expression

(* === Concurrency === *)
actor_decl      := 'actor' UPPER_IDENT '(' params ')' '{' actor_body* '}'
actor_body      := state_decl | receive_decl | fun_stmt
state_decl      := 'state' IDENT '=' expression
receive_decl    := 'receive' UPPER_IDENT ('(' params ')')? block

(* === Helpers === *)
block           := '{' statement* expression? '}'
args            := (expression (',' expression)* ','?)?
where_clause    := 'where' IDENT '=' expression
module_path     := UPPER_IDENT ('.' UPPER_IDENT)*
literal         := INTEGER | FLOAT | STRING | 'true' | 'false' | 'None'
```

---

## 14. Complete Example

A program using most language features:

```flux
//! A simple task manager demonstrating Flux syntax

import Modules.Db as Db

// Types
type Priority = Low | Medium | High | Critical

type Task = {
  id: Int,
  title: String,
  priority: Priority,
  done: Bool,
}

type Filter = All | Active | Completed | ByPriority(Priority)

// Traits
trait Show {
  fun show(self): String
}

impl Show for Priority {
  fun show(self) = match self {
    Low => "low",
    Medium => "medium",
    High => "high",
    Critical => "CRITICAL",
  }
}

impl Show for Task {
  fun show(self) {
    let status = if self.done { "x" } else { " " }
    "[#{status}] #{self.title} (#{show(self.priority)})"
  }
}

// Pure functions
fun create(id, title, priority = Medium) =
  Task { id, title, priority, done: false }

fun toggle(task) =
  { ...task, done: !task.done }

fun visible(tasks, filter) = match filter {
  All => tasks,
  Active => tasks |> filter(\t -> !t.done),
  Completed => tasks |> filter(\t -> t.done),
  ByPriority(p) => tasks |> filter(\t -> t.priority == p),
}

fun summary(tasks) {
  let total = len(tasks)
  let done = tasks |> filter(\t -> t.done) |> len
  let urgent = tasks
    |> filter(\t -> !t.done && t.priority == Critical)
    |> len
  { total, done, urgent }
}
  where remaining = total - done

fun sorted_by_priority(tasks) =
  tasks |> sort(\a, b -> priority_rank(a.priority) - priority_rank(b.priority))
  where priority_rank = \p -> match p {
    Critical => 0,
    High => 1,
    Medium => 2,
    Low => 3,
  }

// Effectful function
fun display(tasks, filter) with IO {
  let shown = tasks |> visible(filter) |> sorted_by_priority
  let stats = summary(tasks)

  print("Tasks (#{stats.done}/#{stats.total} done, #{stats.urgent} urgent):")
  print("---")
  shown |> each(\t -> print(show(t)))
}

// Async operations
fun sync_tasks(tasks) with IO, Async, Fail<DbError> {
  let futures = tasks |> map(\t -> async Db.save(t))
  let results = futures |> map(\f -> await f)

  let errors = [e for Err(e) in results]
  if len(errors) > 0 {
    fail(DbError("#{len(errors)} tasks failed to sync"))
  }
}

// Actor for background processing
actor TaskNotifier {
  state subscribers = []

  receive Subscribe(callback) {
    subscribers = push(subscribers, callback)
  }

  receive Notify(task: Task) {
    subscribers |> each(\cb -> cb(task))
  }
}

// Entry point
fun main() with IO, Async {
  let tasks = [
    create(1, "Learn Flux", High),
    create(2, "Build something", Medium),
    create(3, "Fix critical bug", Critical),
    create(4, "Write docs", Low),
  ]

  let updated = tasks
    |> map(\t -> if t.id == 1 { toggle(t) } else { t })

  display(updated, Active)

  // Sync in background
  let result = try { sync_tasks(updated) }
  match result {
    Ok(_) => print("Synced!"),
    Err(e) => print("Sync failed: #{show(e)}"),
  }
}
```

---

## 15. Migration Notes

### Breaking Changes from Current Syntax

| Current | New | Reason |
|---------|-----|--------|
| `->` in match arms | `=>` | Distinguish from lambda `->` and type arrows `->` |
| `Some`/`None` as keywords | `Some`/`None` as ADT constructors | Generalized via `type Option<T> = Some(T) \| None` |
| `Left`/`Right` as keywords | `Ok`/`Err` convention (or user-defined) | Replace with proper Result type |
| Semicolons (optional) | No semicolons | Expression-based, newline-separated |
| `Assign` statement | Removed | Pure FP — no reassignment (except inside actors) |

### Backward Compatibility Strategy

1. **Edition system** (like Rust) — `edition = "2026"` in project config
2. **Migration tool** — `flux migrate` rewrites `->` to `=>` in match arms, etc.
3. **Deprecation warnings** — old syntax works for one edition, warns, then errors

## References

- [Rust Reference](https://doc.rust-lang.org/reference/) — Pattern matching, traits, match syntax
- [Elm Guide](https://guide.elm-lang.org/) — Pure FP syntax, no side effects
- [Gleam Language Tour](https://gleam.run/book/tour/) — Friendly FP syntax, Result types
- [Koka Documentation](https://koka-lang.github.io/koka/doc/index.html) — Effect system syntax
- [F# Language Reference](https://learn.microsoft.com/en-us/dotnet/fsharp/) — Pipe operator, computation expressions
