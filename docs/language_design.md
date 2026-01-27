# Flux Language Design Document

> A functional, immutable programming language with a custom bytecode VM

## Overview

**Flux** is designed for clarity, safety, and functional programming. Inspired by Elm’s human-friendly compiler errors and Elixir’s expressiveness, it aims to provide a clean syntax with powerful functional features.

The name reflects data flowing through pipelines — the core of functional programming.

**File extension:** `.flx`

## Goals

- **Functional first** — Functions are first-class citizens, higher-order functions everywhere
- **Immutable by default** — No mutable state, no side-effect surprises
- **No null** — Option types instead of billion-dollar mistakes
- **Clean syntax** — Familiar brace-style, minimal boilerplate
- **Safe** — Catch errors early (static types in future versions)
- **Human-friendly errors** — Elm-style error messages that teach, not frustrate

## Version Roadmap

| Version | Features |
|---------|----------|
| v1 | Dynamic types, full pipeline (Lexer → Parser → Compiler → VM), Elm-style errors, compile-time execution |
| v2 | Gradual types, effect system, first-class pipelines, reactive streams, auto-memoization |
| v3 | Full type inference, generics, traits, distributed computing |

---

## Syntax

### Modules

All code lives in modules. Modules provide namespacing and organization.

- Module names must start with an uppercase letter.
- Functions are public by default; prefix with `_` to make them private (not exported).
- Module functions are accessed via `Module.function` and do not leak into the outer scope.
- A module cannot define a function with the same name as the module.

```
// math.flx
module Math {
  // public
  fun square(x) {
    x * x;
  }
  
  // public
  fun cube(x) {
    x * square(x);
  }

  fun call_another_function() {
    print(cube(100));
  }

  fun _private_function() {
    print("cannot be called");
  }
}

// main.flx file
import Math

module Main {
  fun main() {
    print(Math.square(5));
    print(Math._private_function()); // error fun is private
  }
}
```

### Imports

Flexible import system for accessing code from other modules.

Imports are only allowed at the top level (module scope), not inside functions.
Importing a name that already exists in the current scope is an error.

```
// Full mod import
import Math
Math.square(5);      // use with prefix
```

Note: selective imports, aliases, and nested imports are planned but not implemented yet.

### Error Codes

Flux emits human-friendly diagnostics with stable error codes.

| Code | Title | Example | Example file |
| --- | --- | --- | --- |
| E001 | DUPLICATE NAME | `let x = 1; let x = 2;` | `examples/function_redeclaration_error.flx` |
| E003 | IMMUTABLE BINDING | `let x = 1; x = 2;` | — |
| E004 | OUTER ASSIGNMENT | `let x = 1; let f = fun() { x = 2; };` | `examples/closure_outer_assign_error.flx` |
| E007 | UNDEFINED VARIABLE | `print(leng(items));` | — |
| E010 | UNKNOWN PREFIX OPERATOR | `!~x` | — |
| E011 | UNKNOWN INFIX OPERATOR | `1 ^^ 2` | — |
| E012 | DUPLICATE PARAMETER | `fun f(x, x) { x }` | `examples/duplicate_params_error.flx` |
| E016 | INVALID MODULE NAME | `module math { }` | `examples/module_name_lowercase_error.flx` |
| E018 | MODULE NAME CLASH | `module Math { fun Math() {} }` | `examples/module_name_clobber_error.flx` |
| E019 | INVALID MODULE CONTENT | `module Math { let x = 1; }` | — |
| E021 | PRIVATE MEMBER | `Math._private()` | — |
| E030 | IMPORT NAME COLLISION | `let Math = 1; import Math` | `examples/import_collision_error.flx` |
| E031 | IMPORT SCOPE | `fun main() { import Math }` | `examples/import_in_function_error.flx` |
| E032 | IMPORT NOT FOUND | `import Missing` | — |
| E033 | IMPORT READ FAILED | `import Broken` | — |
| E101 | UNKNOWN KEYWORD | `fn main() {}` | `examples/unknown_keyword_fn_error.flx` |
| E102 | EXPECTED EXPRESSION | `;` | `examples/import_semicolon_error.flx` |
| E103 | INVALID INTEGER | `let x = 12_3z;` | — |
| E104 | INVALID FLOAT | `let x = 1.2.3;` | — |
| E105 | UNEXPECTED TOKEN | `print((1 + 2);` | `examples/expected_token_error.flx` |

### Functions

Functions are defined with `fun`. The last expression is the return value.
Function names must be unique within the same scope.
Parameter names must be unique.

```
// Named function
fun add(a, b) {
  a + b;
}

// Anonymous function (lambda)
let double = fun(x) { x * 2; };

// Higher-order function
fun apply_twice(f, x) {
  f(f(x));
}

// Calling functions from functions
fun sum_of_squares(a, b) {
  add(square(a), square(b));
}
```

### Variables

All bindings are immutable. Use `let` to bind values.
Closures cannot assign to outer bindings; use `let` to shadow instead.

```
let name = "Alice";
let age = 30;
let pi = 3.14159;
let active = true;
```

### Comments

C-style single-line comments.

```
// This is a comment
let x = 5;  // Inline comment
```

### Semicolons

Required to terminate statements.

```
let a = 1;
let b = 2;
let c = a + b;
```

---

## Data Types

### Primitives

| Type | Example | Description |
|------|---------|-------------|
| Int | `42`, `-17`, `0` | 64-bit signed integer |
| Float | `3.14`, `-0.5`, `100.0` | 64-bit floating point |
| String | `"hello"`, `"world"` | UTF-8 string |
| Bool | `true`, `false` | Boolean |

### Collections

#### Lists

Ordered, homogeneous collections.

```
let numbers = [1, 2, 3, 4, 5];
let empty = [];
let nested = [[1, 2], [3, 4]];
```

#### Tuples

Fixed-size, heterogeneous collections.

```
let point = (10, 20);
let person = ("Alice", 30, true);
let unit = ();
```

#### Maps

Key-value collections with string keys.

```
let user = {
  "name": "Alice",
  "age": 30,
  "active": true
};

let empty_map = {};
```

### Structs

Named data structures with typed fields.

```
// Define a struct
struct User {
  name: String,
  age: Int,
  active: Bool,
}

struct Point {
  x: Float,
  y: Float,
}

// Create instance
let user = User {
  name: "Alice",
  age: 30,
  active: true,
};

let origin = Point { x: 0.0, y: 0.0 };

// Access fields with dot notation
print(user.name);
print(origin.x);

// Pattern matching on structs
match user {
  User { name, age, active: true } -> print(name);
  User { name, active: false, .. } -> print("inactive user");
}

// Immutable update with spread operator
let older_user = User { ...user, age: user.age + 1 };
let moved = Point { ...origin, x: 10.0 };
```

Structs vs Maps:
- **Structs**: Fixed shape, typed fields, compile-time checks (in v2+)
- **Maps**: Dynamic keys, flexible shape, runtime access

### Enums (Sum Types)

Algebraic data types for modeling variants.

```flux
// Simple enum
enum Status {
  Active,
  Inactive,
  Pending,
}

// Enum with data (tagged union)
enum Option {
  Some(value),
  None,
}

enum Result {
  Ok(value),
  Err(message),
}

// Complex variants with named fields
enum Shape {
  Circle(radius: Float),
  Rectangle(width: Float, height: Float),
  Point,
}

// Generic enums (v2+)
enum List<T> {
  Cons(head: T, tail: List<T>),
  Nil,
}
```

**Usage:**

```flux
// Create enum values
let status = Active;
let maybe = Some(42);
let result = Ok("success");
let shape = Circle(5.0);

// Pattern matching on enums
match maybe {
  Some(x) -> print(x);
  None -> print("nothing");
}

match shape {
  Circle(r) -> 3.14 * r * r;
  Rectangle(w, h) -> w * h;
  Point -> 0.0;
}

// Nested matching
match result {
  Ok(value) -> match do_something(value) {
    Ok(v) -> v;
    Err(e) -> handle_error(e);
  };
  Err(msg) -> print(msg);
}
```

**Built-in Enums:**

```flux
// Option - replaces null
enum Option {
  Some(value),
  None,
}

// Result - error handling
enum Result {
  Ok(value),
  Err(error),
}
```

Structs + Enums = Full Algebraic Data Types:
- **Structs (Product types)**: A AND B AND C
- **Enums (Sum types)**: A OR B OR C

---

## Operators

### Arithmetic

| Operator | Description | Example |
|----------|-------------|---------|
| `+` | Addition | `1 + 2` → `3` |
| `-` | Subtraction | `5 - 3` → `2` |
| `*` | Multiplication | `4 * 3` → `12` |
| `/` | Division | `10 / 2` → `5` |

### Comparison

| Operator | Description | Example |
|----------|-------------|---------|
| `==` | Equal | `1 == 1` → `true` |
| `!=` | Not equal | `1 != 2` → `true` |
| `<` | Less than | `1 < 2` → `true` |
| `>` | Greater than | `2 > 1` → `true` |
| `<=` | Less or equal | `1 <= 1` → `true` |
| `>=` | Greater or equal | `2 >= 1` → `true` |

### Logical

| Operator | Description | Example |
|----------|-------------|---------|
| `!` | Not | `!true` → `false` |
| `&&` | And | `true && false` → `false` |
| `\|\|` | Or | `true \|\| false` → `true` |

### Special

| Operator | Description | Example |
|----------|-------------|---------|
| `\|>` | Pipe | `x \|> f \|> g` |
| `.` | Module access | `Math.square(5)` |
| `->` | Match arm | `1 -> "one"` |

---

## Control Flow

### Conditionals

```
if x > 0 {
  print("positive");
} else if x < 0 {
  print("negative");
} else {
  print("zero");
}

// Conditional as expression
let abs = if x >= 0 { x; } else { -x; };
```

### Pattern Matching

Powerful pattern matching with `match`.

```
// Simple value matching
match x {
  0 -> "zero";
  1 -> "one";
  _ -> "other";
}

// With destructuring (future)
match point {
  (0, 0) -> "origin";
  (x, 0) -> "on x-axis";
  (0, y) -> "on y-axis";
  (x, y) -> "somewhere";
}

// List patterns (future)
match list {
  [] -> "empty";
  [x] -> "single";
  [x, y] -> "pair";
  _ -> "many";
}
```

### List Comprehensions

Functional iteration that returns new lists. No traditional loops.

```
// Simple transform
let squares = for x in numbers { x * x; };

// With filter condition
let big_ones = for x in numbers, x > 10 { x; };

// Multiple generators (nested iteration)
let pairs = for x in [1, 2], y in ["a", "b"] { (x, y); };
// Result: [(1, "a"), (1, "b"), (2, "a"), (2, "b")]

// Multiple conditions
let result = for x in numbers, x > 0, x < 100 { x * 2; };

// With pattern matching
let names = for (name, _age) in people { name; };

// Nested comprehension
let matrix = for row in rows {
  for col in cols { row * col; };
};
```

### Recursion

For complex iteration, use recursion with pattern matching.

```
fun sum(list) {
  match list {
    [] -> 0;
    [head, ...tail] -> head + sum(tail);
  }
}

fun factorial(n) {
  match n {
    0 -> 1;
    _ -> n * factorial(n - 1);
  }
}

fun map(list, f) {
  match list {
    [] -> [];
    [head, ...tail] -> [f(head), ...map(tail, f)];
  }
}
```

---

## Pipelines

Chain function calls left-to-right.

```
// Without pipeline
print(filter(map(numbers, double), is_even));

// With pipeline
numbers
  |> map(fun(x) { x * 2; })
  |> filter(fun(x) { x > 4; })
  |> print;
```

---

## No Null

This language has no `null` or `nil`. Optional values use the `Option` type.

```
// Option type (built-in)
let some_value = Some(42);
let no_value = None;

// Must handle both cases
match some_value {
  Some(x) -> print(x);
  None -> print("no value");
}

// Functions that might fail return Option
match get(map, "key") {
  Some(value) -> use(value);
  None -> handle_missing();
}
```

---

## Standard Library (Planned)

### List Functions

```
head(list)              // Option - first element
tail(list)              // List - all but first
length(list)            // Int - count elements
map(list, fun)           // List - transform each
filter(list, fun)        // List - keep matching
reduce(list, init, fun)  // Any - fold into value
concat(list1, list2)    // List - join lists
reverse(list)           // List - reverse order
```

### Map Functions

```
get(map, key)           // Option - get value
keys(map)               // List - all keys
values(map)             // List - all values
put(map, key, value)    // Map - new map with entry
delete(map, key)        // Map - new map without entry
```

### Tuple Functions

```
first(tuple)            // Any - first element
second(tuple)           // Any - second element  
tuple_to_list(tuple)    // List - convert
```

### I/O Functions

```
print(value)            // Unit - output to stdout
input()                 // String - read from stdin
```

---

## Compile-Time Execution

Like Jai, Flux can run any code at compile time using `#` directives.

### #run — Execute at Compile Time

```flx
// Print during compilation
#run print("Compiling...");

// Compute values at compile time
let lookup_table = #run {
  generate_lookup_table(1000);
};

// Complex compile-time logic
#run {
  let config = parse_config("build.json");
  if config.debug {
    print("Debug build");
  };
};
```

### #assert — Compile-Time Assertions

```flx
// Static assertions
#assert size_of(User) < 64;
#assert VERSION > 2;

// Validate constants
const MAX_SIZE = 1024;
#assert MAX_SIZE > 0;
```

### #if / #else — Conditional Compilation

```flx
#if DEBUG {
  fun log(msg) with IO {
    print("[DEBUG] " + msg);
  }
} #else {
  fun log(msg) {
    // no-op in release
  }
}

#if PLATFORM == "windows" {
  fun path_separator() { "\\"; }
} #else {
  fun path_separator() { "/"; }
}
```

### #emit — Generate Code

```flx
// Generate functions at compile time
#run {
  for i in 0..5 {
    #emit fun get_{i}() { {i}; };
  }
}

// Generates:
// fun get_0() { 0; }
// fun get_1() { 1; }
// ...
```

### Use Cases

| Directive | Use Case |
|-----------|----------|
| `#run` | Precompute tables, validate config, code generation |
| `#assert` | Static guarantees, size checks, invariants |
| `#if/#else` | Platform-specific code, debug/release builds |
| `#emit` | Metaprogramming, boilerplate generation |

---

## Error Messages

Flux follows Elm's philosophy: **errors are for humans, not compiler engineers**.

### Principles

1. **Say what went wrong** — In plain English
2. **Show where it happened** — With code context
3. **Explain why** — Help understand the problem
4. **Suggest a fix** — Offer concrete solutions
5. **Use color** — Visual hierarchy aids reading

### Examples

**Type Mismatch:**

```
── TYPE MISMATCH ─────────────────────────────── src/main.flx:12:15 ──

I was expecting a `String`, but found an `Int`.

 12 │   let name = 42;
    │              ^^
 13 │   greet(name);
    │         ^^^^

The function `greet` expects its argument to be:

    String

But you gave it:

    Int

Hint: Try converting with `to_string`:

    greet(to_string(name));
```

**Missing Field:**

```
── MISSING FIELD ─────────────────────────────── src/main.flx:8:3 ──

This `User` struct is missing the `email` field.

  8 │   let user = User {
  9 │     name: "Alice",
 10 │     age: 30,
 11 │   };
    │   ^

I was expecting to see all these fields:

    name  ✓ found
    age   ✓ found
    email ✗ missing

Hint: Add the missing field:

    let user = User {
      name: "Alice",
      age: 30,
      email: "alice@example.com",
    };
```

**Pattern Match Not Exhaustive:**

```
── INCOMPLETE PATTERN ────────────────────────── src/main.flx:15:3 ──

This `match` doesn't cover all possibilities.

 15 │   match result {
 16 │     Ok(value) -> value;
 17 │   };

You handled:

    ✓ Ok(value)

But you forgot:

    ✗ Err(_)

Hint: Add the missing case:

    match result {
      Ok(value) -> value;
      Err(e) -> handle_error(e);
    };

Or use a wildcard if you want to ignore it:

    match result {
      Ok(value) -> value;
      _ -> default_value;
    };
```

**Unknown Variable:**

```
── UNKNOWN NAME ──────────────────────────────── src/main.flx:5:10 ──

I don't recognize the name `username`.

  5 │   print(username);
    │         ^^^^^^^^

Did you mean one of these?

    user_name  (defined on line 3)
    user       (defined on line 2)
    name       (defined on line 1)
```

**Wrong Number of Arguments:**

```
── ARGUMENT COUNT ────────────────────────────── src/main.flx:10:3 ──

The function `add` expects 2 arguments, but you gave it 3.

 10 │   add(1, 2, 3);
    │   ^^^^^^^^^^^

The definition of `add` is:

  2 │   fun add(a, b) {
    │          ^^^^
    │          expects 2 arguments

Hint: Remove the extra argument:

    add(1, 2);
```

**Effect Violation (v2):**

```
── EFFECT NOT ALLOWED ────────────────────────── src/main.flx:7:5 ──

You're trying to use `print` in a pure function.

  5 │   fun calculate(x) {
  6 │     let result = x * 2;
  7 │     print(result);
    │     ^^^^^^^^^^^^^
  8 │     result;
  9 │   }

The function `calculate` is pure (no effects), but `print` requires:

    IO effect

You have two options:

1. Add the effect to your function:

    fun calculate(x) with IO {
      ...
    }

2. Remove the side effect (recommended for pure calculations):

    fun calculate(x) {
      let result = x * 2;
      result;
    }
```

**Unused Variable:**

```
── UNUSED VARIABLE ───────────────────────────── src/main.flx:3:7 ──

You declared `temp` but never used it.

  3 │   let temp = calculate();
    │       ^^^^
  4 │   let result = other_thing();
  5 │   result;

Hint: If this is intentional, prefix with underscore:

    let _temp = calculate();

Or did you mean to use it somewhere?
```

**Infinite Recursion Warning:**

```
── POSSIBLE INFINITE LOOP ────────────────────── src/main.flx:2:3 ──

This function might never terminate.

  2 │   fun countdown(n) {
  3 │     countdown(n - 1);
  4 │   }

I don't see a base case that stops the recursion.

Hint: Add a terminating condition:

    fun countdown(n) {
      match n {
        0 -> print("Done!");
        _ -> countdown(n - 1);
      }
    }
```

**Import Not Found:**

```
── IMPORT NOT FOUND ──────────────────────────── src/main.flx:1:8 ──

I can't find a mod called `Maths`.

  1 │ import Maths;
           ^^^^^

Did you mean?

    Math       (standard library)
    MyMath     (in ./src/mymath.flx)

Available modules:

    Math, String, List, Map, IO
```

### Error Message Guidelines

| ✅ Do | ❌ Don't |
|-------|----------|
| "I was expecting X" | "Expected X" |
| "Did you mean...?" | "Undefined reference" |
| Show the exact location | Just show line number |
| Suggest a fix | Leave user stuck |
| Use plain language | Use jargon |
| One problem at a time | Dump all errors |

### Color Scheme

```
Red       → Error location, problem
Yellow    → Warning, caution
Cyan      → Code snippets
Green     → Suggestions, fixes
White     → Explanatory text
Bold      → Important terms
```

---

```
┌─────────────────┐
│   Source Code   │  .flx files
└────────┬────────┘
         │
    ┌────▼────┐
    │  Lexer  │      Source → Tokens
    └────┬────┘
         │
    ┌────▼────┐
    │ Parser  │      Tokens → AST
    └────┬────┘
         │
    ┌────▼────┐
    │Compiler │      AST → Bytecode
    └────┬────┘
         │
    ┌────▼────┐
    │   VM    │      Execute bytecode
    └─────────┘
```

### Bytecode VM

Stack-based virtual machine executing custom bytecode.

```
// Example: let x = 1 + 2;

OpConstant 0    // push 1
OpConstant 1    // push 2  
OpAdd           // pop, pop, push 3
OpSetGlobal 0   // store as x
```

---

## Grammar (EBNF)

Current parser grammar (v1):

```ebnf
program        = statement* ;

statement      = module_stmt
               | import_stmt
               | function_stmt
               | let_stmt
               | assign_stmt
               | return_stmt
               | expr_stmt ;

module_stmt    = "module" IDENT block ;
import_stmt    = "import" IDENT ;
function_stmt  = "fun" IDENT "(" parameters? ")" block ;
let_stmt       = "let" IDENT "=" expression ";"? ;
assign_stmt    = IDENT "=" expression ";"? ;
return_stmt    = "return" expression? ";"? ;
expr_stmt      = expression ";"? ;

parameters     = IDENT ( "," IDENT )* ;
block          = "{" statement* "}" ;

expression     = equality ;
equality       = comparison ( ( "==" | "!=" ) comparison )* ;
comparison     = term ( ( "<" | ">" ) term )* ;
term           = factor ( ( "+" | "-" ) factor )* ;
factor         = unary ( ( "*" | "/" ) unary )* ;
unary          = ( "!" | "-" ) unary
               | postfix ;

postfix        = primary ( "(" arguments? ")"
                         | "[" expression "]"
                         | "." IDENT )* ;

arguments      = expression ( "," expression )* ;

primary        = INT | FLOAT | STRING | "true" | "false" | "null"
               | IDENT
               | "(" expression ")"
               | "[" arguments? "]"
               | "{" hash_items? "}"
               | "fun" "(" parameters? ")" block
               | if_expr ;

if_expr        = "if" expression block ( "else" block )? ;

hash_items     = expression ":" expression ( "," expression ":" expression )* ;
```

Note: structs/enums, match, for, pipelines, directives, and types are planned but not implemented yet.

---

## Example Program

```
import Math

module Main {
  fun main() {
    let numbers = [1, 2, 3, 4, 5];
    let squared = fun(x) { x * x };
    let first = numbers[0];

    if first > 0 {
      print(Math.square(first));
    } else {
      print(0);
    }
  }
}
```

---

## Future Considerations

### v2: Gradual Types + Effect System

```flux
// Type annotations
fun add(a: Int, b: Int) -> Int {
  a + b;
}

// Untyped still works
fun double(x) {
  x * 2;
}
```

**Effect System:**

Pure functions by default, explicit effect annotations for side effects.

```flux
// Pure - no annotation needed, compiler enforces no effects
fun add(a, b) {
  a + b;
}

// Impure - must declare effects
fun greet(name) with IO {
  print("Hello " + name);
}

// Effects propagate
fun greet_twice(name) with IO {
  greet(name);
  greet(name);
}

// Multiple effects
fun fetch_and_log(url) with IO, Async {
  let data = http_get(url);
  print(data);
}

// main allows IO
fun main() with IO {
  let x = add(1, 2);   // pure - works anywhere
  greet("Alice");       // IO - only in IO context
}
```

Effect types:
- `IO` — Console, file system, network
- `Async` — Asynchronous operations
- `Random` — Non-deterministic values
- `Error` — Recoverable failures

### v2: First-Class Pipelines

Pipelines are reified data structures — save, compose, debug, parallelize.

```flux
// Define a pipeline as a value
let process = pipeline {
  map(fun(x) { x * 2; })
  |> filter(fun(x) { x > 10; })
  |> reduce(0, fun(acc, x) { acc + x; })
};

// Apply it
let result = data |> process;

// Compose pipelines
let enhanced = pipeline {
  validate
  |> process
  |> format
};

// Debug step-by-step
let traced = process |> debug;
data |> traced;
// Output:
// Step 1 (map): [2, 4, 6, 8, 10, 12, 14, 16, 18, 20]
// Step 2 (filter): [12, 14, 16, 18, 20]
// Step 3 (reduce): 80

// Automatic parallelization
let fast = process |> parallel;
big_data |> fast;  // Runs map/filter across cores
```

### v2: Reactive Streams

Async data with the same syntax as sync.

```flux
// Create a stream
let clicks = stream(document.onClick);
let ticks = interval(1000);

// Process with familiar syntax
let processed = clicks
  |> filter(fun(e) { e.target == button; })
  |> debounce(300)
  |> map(fun(e) { e.position; });

// Combine streams
let combined = merge(clicks, ticks);
let synced = zip(userInput, serverResponse);

// Same pipeline works on lists AND streams
let transform = pipeline {
  filter(fun(x) { x.valid; })
  |> map(process)
};

list |> transform;    // sync
stream |> transform;  // async
```

### v2: Automatic Memoization

Pure functions get automatic caching.

```flux
// Mark function as memoized
@memo
fun fibonacci(n) {
  match n {
    0 -> 0;
    1 -> 1;
    _ -> fibonacci(n - 1) + fibonacci(n - 2);
  }
}

// Now O(n) instead of O(2^n)
fibonacci(100);  // instant

// Smart cache policies
@memo(max: 1000, ttl: 3600)
fun expensive_calculation(input) {
  // complex computation
}

// Cache invalidation via effects
@memo(invalidate_on: [UserUpdated])
fun get_user_data(id) with IO {
  fetch_from_db(id);
}
```

### v3: Advanced Features

- Type inference
- Generics
- Traits/Typeclasses
- Concurrency primitives
- Module imports/exports

---

## Design Decisions Log

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Paradigm | Functional | Clean, predictable, composable |
| Mutability | Immutable only | Eliminates state bugs |
| Null | No null, use Option | Billion-dollar mistake avoided |
| Types (v1) | Dynamic | Get pipeline working first |
| Syntax | Brace-style | Familiar, easy to parse |
| Semicolons | Required | Clear statement boundaries |
| Comments | `//` C-style | Familiar to most developers |
| Backend | Bytecode VM | Full control, educational |
| Maps | `{"k": v}` | JSON-familiar |
| Imports | Flexible (Option D) | Full, selective, aliased imports |
| Loops | Comprehensions + recursion | Purely functional, no mutation |
| Structs | Rust-style | Named fields, pattern matching, spread updates |
| ADTs | Structs + Enums | Full algebraic data types |
| Error Messages | Elm-style | Human-friendly, helpful, educational |
| Pipelines (v2) | First-class | Debuggable, parallelizable, composable |
| Streams (v2) | Reactive | Unified sync/async model |
| Caching (v2) | Auto-memoization | Effect system enables smart invalidation |
| File extension | `.flx` | Short, unique |
| Function keyword | `fun` | FUNctional, playful, short |
| Module keyword | `mod` | Short, Rust-familiar |
| Compile-time | `#run`, `#assert`, `#if`, `#emit` | Jai-style metaprogramming |
