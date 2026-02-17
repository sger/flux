# Flux Language Features Proposal

This document outlines proposed language features and syntax improvements for Flux, organized by priority and implementation complexity.

---

## Table of Contents

1. [Design Philosophy](#design-philosophy)
2. [Part I: Syntax Improvements](#part-i-syntax-improvements)
3. [Part II: Core Language Features](#part-ii-core-language-features)
4. [Part III: Advanced Features](#part-iii-advanced-features)
5. [Implementation Roadmap](#implementation-roadmap)

---

## Design Philosophy

Flux aims to be a **functional-first** language with these guiding principles:

1. **Data flows through transformations** - The name "Flux" reflects this
2. **Expressions over statements** - Everything returns a value
3. **Immutability by default** - Explicit when mutation occurs
4. **Effects are tracked** - Side effects are visible in the type system
5. **Concurrency is safe** - Actor model for isolation

### Syntax Style

- **Clean and minimal** - Avoid unnecessary syntax noise
- **Consistent** - Similar constructs look similar
- **Readable** - Code reads like documentation
- **Familiar** - Draw from established FP languages

---

## Part I: Syntax Improvements

### 1.1 Missing Operators (Critical)

#### Comparison Operators: `<=` and `>=`

**Current limitation:**
```flux
// Cannot write this
if n <= 0 { ... }

// Must use workaround
if n < 1 { ... }  // Only works for integers
```

**Proposed syntax:**
```flux
n <= 10    // less than or equal
n >= 0     // greater than or equal
```

**Lexer changes:**
- Add `LessEqual` token for `<=`
- Add `GreaterEqual` token for `>=`

**Parser changes:**
- Parse as infix operators with `LessGreater` precedence

**VM changes:**
- Add `OpLessEqual` and `OpGreaterEqual` opcodes
- Or: compile to `!(a > b)` and `!(a < b)`

---

#### Logical Operators: `&&` and `||`

**Current limitation:**
```flux
// Cannot write this
if a > 0 && b > 0 { ... }

// Must nest if statements
if a > 0 {
    if b > 0 { ... }
}
```

**Proposed syntax:**
```flux
a && b     // logical AND (short-circuit)
a || b     // logical OR (short-circuit)
```

**Key requirement: Short-circuit evaluation**
```flux
// Right side should NOT evaluate if left determines result
false && expensive()  // expensive() not called
true || expensive()   // expensive() not called
```

**Implementation approach:**

Cannot use simple opcodes - requires conditional jumps:

```
// Compile: a && b
evaluate a
OpJumpNotTruthy end_label
OpPop                    // discard 'a' result
evaluate b
end_label:
// Stack now has: false (if a was false) or b's value

// Compile: a || b
evaluate a
OpJumpTruthy end_label
OpPop                    // discard 'a' result
evaluate b
end_label:
// Stack now has: true (if a was true) or b's value
```

---

#### Modulo Operator: `%`

**Current limitation:**
```flux
// Cannot check if even/odd
// Cannot wrap around values
```

**Proposed syntax:**
```flux
10 % 3      // 1
n % 2 == 0  // is even
index % len // wrap around
```

**Implementation:**
- Add `Percent` token
- Add `OpMod` opcode
- Handle both integer and float modulo

---

### 1.2 Pipe Operator `|>` (High Priority)

The most impactful syntax addition for functional programming.

**Current limitation:**
```flux
// Nested calls are hard to read
print(to_string(sum(filter(map(data, transform), is_valid))))

// Reading order: inside-out, right-to-left
```

**Proposed syntax:**
```flux
// Data flows left-to-right, top-to-bottom
data
    |> map(transform)
    |> filter(is_valid)
    |> sum
    |> to_string
    |> print
```

**Semantics:**
```flux
// a |> f(b, c) is equivalent to f(a, b, c)
// The left side becomes the FIRST argument

5 |> add(3)           // add(5, 3)
arr |> map(double)    // map(arr, double)
x |> f |> g |> h      // h(g(f(x)))
```

**Precedence:**
- Lower than all arithmetic/comparison operators
- Left-associative
- `a + b |> f` parses as `(a + b) |> f`

**Token:** `Pipe` for `|>`

**AST:** New `Pipe` expression type or desugar during parsing

**Alternative considered:**
```flux
// Elixir uses same syntax
data |> map(&1 * 2)  // with placeholder

// F# uses same syntax
data |> List.map (fn x -> x * 2)

// Haskell uses & (flip application)
data & map double & filter isValid
```

**Recommendation:** Use `|>` with first-argument insertion (Elixir/F# style)

---

### 1.3 Lambda Shorthand (High Priority)

**Current limitation:**
```flux
// Verbose for simple operations
map(arr, fn(x) { x * 2; })
filter(arr, fn(x) { x > 0; })
reduce(arr, 0, fn(acc, x) { acc + x; })
```

**Proposed options:**

#### Option A: Backslash Arrow (Haskell-inspired)
```flux
map(arr, \x -> x * 2)
filter(arr, \x -> x > 0)
reduce(arr, 0, \acc, x -> acc + x)

// Multi-line
sort_by(users, \user -> {
    let score = calculate_score(user);
    score * user.priority
})
```

#### Option B: Vertical Bars (Rust-inspired)
```flux
map(arr, |x| x * 2)
filter(arr, |x| x > 0)
reduce(arr, 0, |acc, x| acc + x)

// Multi-line with braces
sort_by(users, |user| {
    let score = calculate_score(user);
    score * user.priority
})
```

#### Option C: Underscore Placeholder (Scala-inspired)
```flux
map(arr, _ * 2)
filter(arr, _ > 0)
reduce(arr, 0, _ + _)  // Each _ is a different parameter

// Limited to simple expressions
// Cannot use same parameter twice: _ + _ is two params
```

**Recommendation:** Option A (`\x -> expr`)

**Rationale:**
- Unambiguous (no conflict with bitwise OR)
- Supports multiple parameters naturally
- Visual similarity to mathematical lambda (λ)
- Works well with multi-line bodies

**Grammar:**
```
lambda := '\' parameters '->' expression
        | '\' parameters '->' block
parameters := identifier (',' identifier)*
```

---

### 1.4 Block Comments (Medium Priority)

**Current limitation:**
```flux
// Only single-line comments
// Must repeat // for each line
// No way to comment out large blocks easily
```

**Proposed syntax:**
```flux
/* Block comment
   can span multiple lines
   useful for temporarily disabling code */

/*
 * Documentation style
 * with leading asterisks
 */

/* Nested /* comments */ are supported */
```

**Additional: Doc comments**
```flux
/// Function documentation
/// Supports markdown formatting
///
/// # Examples
/// ```flux
/// add(1, 2)  // returns 3
/// ```
fn add(a, b) {
    a + b
}

//! Module-level documentation
//! Describes the entire module
module Math {
    ...
}
```

**Implementation:**
- Lexer tracks nesting depth for `/*` and `*/`
- Doc comments (`///`, `//!`) captured as metadata
- Block comments can nest (unlike C)

---

### 1.5 Destructuring (High Priority)

**Current limitation:**
```flux
// Cannot extract values directly
let pair = [1, 2];
let first = pair[0];
let second = pair[1];

// Cannot destructure in function params
fn process_point(point) {
    let x = point.x;
    let y = point.y;
    // ...
}
```

**Proposed syntax:**

#### Array Destructuring
```flux
let [first, second] = pair;
let [head, ...tail] = list;        // rest pattern
let [a, _, c] = triple;            // ignore with _
let [x, y, ...] = many;            // ignore rest

// Nested
let [[a, b], [c, d]] = matrix;
```

#### Hash/Record Destructuring
```flux
let { name, age } = person;
let { x, y, ...rest } = point;     // rest pattern
let { name: n, age: a } = person;  // rename

// Nested
let { address: { city, zip } } = user;
```

#### Tuple Destructuring (when tuples added)
```flux
let (x, y) = point;
let (first, _, third) = triple;
let (head, ...tail) = tuple;
```

#### In Function Parameters
```flux
fn distance([x1, y1], [x2, y2]) {
    let dx = x2 - x1;
    let dy = y2 - y1;
    sqrt(dx * dx + dy * dy)
}

fn greet({ name, title }) {
    "Hello, #{title} #{name}!"
}
```

#### In Match Patterns
```flux
match point {
    [0, 0] -> "origin";
    [x, 0] -> "on x-axis at #{x}";
    [0, y] -> "on y-axis at #{y}";
    [x, y] -> "point at (#{x}, #{y})";
}
```

---

### 1.6 Pattern Guards (Medium Priority)

**Current limitation:**
```flux
// Cannot add conditions to patterns
match n {
    // No way to say "x where x > 0"
}
```

**Proposed syntax:**
```flux
match value {
    x if x > 0 -> "positive";
    x if x < 0 -> "negative";
    0 -> "zero";
}

// Multiple conditions
match point {
    (x, y) if x == y -> "on diagonal";
    (x, y) if x == 0 -> "on y-axis";
    (x, y) if y == 0 -> "on x-axis";
    (x, y) -> "general point";
}

// With destructuring
match user {
    { age, name } if age >= 18 -> "Adult: #{name}";
    { age, name } -> "Minor: #{name}";
}

// Guards can use bound variables
match list {
    [head, ...tail] if len(tail) > 0 -> "has more";
    [single] -> "just one";
    [] -> "empty";
}
```

**Semantics:**
- Guard evaluated after pattern matches
- Guard must return boolean
- Can reference variables bound in pattern
- Guards evaluated in order; first true wins

---

### 1.7 Or-Patterns (Low Priority)

**Current limitation:**
```flux
// Must repeat code for similar cases
match day {
    "Monday" -> "weekday";
    "Tuesday" -> "weekday";
    "Wednesday" -> "weekday";
    // ... etc
}
```

**Proposed syntax:**
```flux
match day {
    "Saturday" | "Sunday" -> "weekend";
    _ -> "weekday";
}

match result {
    None | Some(0) -> "empty or zero";
    Some(n) -> "has value: #{n}";
}

// With shared bindings (must bind same names)
match expr {
    Add(a, b) | Sub(a, b) -> process(a, b);
    Mul(x, y) -> multiply(x, y);
}
```

---

### 1.8 Default Parameters (Medium Priority)

**Current limitation:**
```flux
// Cannot provide defaults
fn greet(name, greeting) {
    "#{greeting}, #{name}!"
}
// Must always pass both: greet("World", "Hello")
```

**Proposed syntax:**
```flux
fn greet(name, greeting = "Hello") {
    "#{greeting}, #{name}!"
}

greet("World")           // "Hello, World!"
greet("World", "Hi")     // "Hi, World!"

// Multiple defaults
fn create_user(name, age = 0, active = true) {
    { name: name, age: age, active: active }
}

create_user("Alice")              // age=0, active=true
create_user("Bob", 25)            // active=true
create_user("Carol", 30, false)   // all specified
```

**Rules:**
- Default parameters must come after required ones
- Defaults evaluated at call time (not definition time)
- Defaults can reference earlier parameters

```flux
fn range(start, end, step = 1) { ... }
fn pad(str, width, char = " ") { ... }
```

---

### 1.9 Named Parameters (Low Priority)

**Current limitation:**
```flux
// Parameter order must be memorized
create_user("Alice", 25, true, false, "admin")
// What do these booleans mean?
```

**Proposed syntax:**
```flux
// At call site, use parameter names
create_user(
    name: "Alice",
    age: 25,
    active: true,
    verified: false,
    role: "admin"
)

// Can mix positional and named (positional first)
create_user("Alice", 25, role: "admin")

// Named can be in any order
http_request(
    method: "POST",
    url: "/api/users",
    body: data,
    timeout: 5000
)
```

**Rules:**
- Positional arguments must come before named
- Named arguments can be in any order
- Cannot specify same argument twice

---

### 1.10 Range Syntax (Medium Priority)

**Proposed syntax:**
```flux
// Exclusive range (end not included)
1..10        // 1, 2, 3, 4, 5, 6, 7, 8, 9

// Inclusive range
1..=10       // 1, 2, 3, 4, 5, 6, 7, 8, 9, 10

// With step
0..10..2     // 0, 2, 4, 6, 8
10..0..-1    // 10, 9, 8, ..., 1

// Open ranges (when context provides bounds)
..10         // 0 to 9
5..          // 5 to end

// Usage
for i in 1..=5 {
    print(i)
}

let slice = arr[2..5]    // elements 2, 3, 4
let chars = str[0..3]    // first 3 characters
```

**Implementation:**
- `Range` object type: `{ start, end, step, inclusive }`
- Lazy evaluation (doesn't create full list)
- Works with `for`, array slicing, list comprehensions

---

### 1.11 Expression Enhancements

#### If-Let (Pattern Matching in Conditions)
```flux
// Current: nested match
match get_user(id) {
    Some(user) -> {
        print(user.name);
    };
    None -> {};
}

// Proposed: if-let
if let Some(user) = get_user(id) {
    print(user.name);
}

// With else
if let Some(value) = maybe_value {
    process(value)
} else {
    default_action()
}

// Chained
if let Some(user) = get_user(id) {
    if let Some(email) = user.email {
        send_notification(email)
    }
}
```

#### Let-Else (Early Return)
```flux
// Current: nested matches or manual checks
fn process(data) {
    match validate(data) {
        Some(valid) -> {
            // continue with valid...
        };
        None -> {
            return None;
        };
    }
}

// Proposed: let-else
fn process(data) {
    let Some(valid) = validate(data) else {
        return None;
    };

    // continue with valid...
    Some(transform(valid))
}
```

---

## Part II: Core Language Features

### 2.1 Algebraic Data Types (ADTs)

User-defined sum types - the foundation for domain modeling.

**Proposed syntax:**

#### Type Declaration
```flux
type Option<T> {
    None
    Some(T)
}

type Either<L, R> {
    Left(L)
    Right(R)
}

type List<T> {
    Nil
    Cons(T, List<T>)
}

// Without generics initially
type Shape {
    Circle(radius)
    Rectangle(width, height)
    Triangle(a, b, c)
}

type Result {
    Ok(value)
    Err(message)
}
```

#### Construction
```flux
let shape = Circle(5.0);
let result = Ok(42);
let list = Cons(1, Cons(2, Cons(3, Nil)));
```

#### Pattern Matching
```flux
match shape {
    Circle(r) -> 3.14159 * r * r;
    Rectangle(w, h) -> w * h;
    Triangle(a, b, c) -> {
        let s = (a + b + c) / 2;
        sqrt(s * (s-a) * (s-b) * (s-c))
    };
}
```

#### With Type Parameters (Future)
```flux
type Tree<T> {
    Leaf(T)
    Node(Tree<T>, T, Tree<T>)
}

fn map_tree<T, U>(tree: Tree<T>, f: T -> U): Tree<U> {
    match tree {
        Leaf(x) -> Leaf(f(x));
        Node(left, x, right) ->
            Node(map_tree(left, f), f(x), map_tree(right, f));
    }
}
```

---

### 2.2 Record Types (Structs)

Named fields with optional types.

**Proposed syntax:**

#### Declaration
```flux
type Point {
    x: Float,
    y: Float
}

type User {
    name: String,
    email: String,
    age: Int,
    active: Bool = true  // default value
}

// Without explicit types (inferred)
type Config {
    host,
    port,
    timeout = 5000
}
```

#### Construction
```flux
let p = Point { x: 10.0, y: 20.0 };
let user = User {
    name: "Alice",
    email: "alice@example.com",
    age: 25
    // active defaults to true
};

// Shorthand when variable name matches field
let x = 10.0;
let y = 20.0;
let p = Point { x, y };  // same as Point { x: x, y: y }
```

#### Access and Update
```flux
let name = user.name;

// Functional update (creates new record)
let older = { user | age: user.age + 1 };
let moved = { point | x: point.x + dx, y: point.y + dy };
```

#### Destructuring
```flux
let Point { x, y } = point;
let User { name, age, ... } = user;  // ignore other fields

fn distance(Point { x: x1, y: y1 }, Point { x: x2, y: y2 }) {
    sqrt((x2-x1)^2 + (y2-y1)^2)
}
```

---

### 2.3 Tuple Type

Fixed-size, heterogeneous collections.

**Proposed syntax:**

#### Literals
```flux
let pair = (1, "hello");
let triple = (1, 2.0, true);
let unit = ();
let single = (42,);  // trailing comma for single-element
```

#### Access
```flux
// By index (0-based)
let first = pair.0;
let second = pair.1;

// Destructuring
let (x, y) = pair;
let (a, b, c) = triple;
let (head, ...rest) = many;
```

#### Type Annotation (Future)
```flux
let point: (Int, Int) = (10, 20);

fn swap<A, B>(pair: (A, B)): (B, A) {
    let (a, b) = pair;
    (b, a)
}
```

---

### 2.4 List Comprehensions

Concise syntax for generating and transforming lists.

**Proposed syntax:**

#### Basic Form
```flux
// Python-style
[x * 2 for x in arr]
[x for x in arr if x > 0]

// Or Haskell-style
[x * 2 | x <- arr]
[x | x <- arr, x > 0]
```

**Recommendation:** Python-style (more readable for newcomers)

#### With Multiple Generators
```flux
// Cartesian product
[(x, y) for x in xs for y in ys]

// Nested with filter
[x + y for x in xs if x > 0 for y in ys if y > 0]

// Equivalent to:
xs |> flat_map(\x ->
    if x > 0 {
        ys |> filter(\y -> y > 0)
           |> map(\y -> x + y)
    } else {
        []
    }
)
```

#### With Pattern Matching
```flux
[name for { name, age } in users if age >= 18]
[x for Some(x) in options]  // filter and unwrap
```

#### Hash Comprehensions
```flux
// Create hash from list
{ k: v for (k, v) in pairs }
{ name: len(name) for name in names }
```

---

## Part III: Advanced Features

### 3.1 Effect System

Track side effects in the type system, making impure code explicit.

#### Design Goals
- Pure functions by default
- Effects are visible in signatures
- Effects can be handled/mocked
- Composable effect tracking

#### Core Concepts

**Built-in Effects:**
```flux
effect IO          // Console, file, network
effect State<S>    // Mutable state of type S
effect Async       // Asynchronous operations
effect Fail<E>     // Can fail with error E
effect Random      // Random number generation
effect Time        // Current time, delays
```

**Effect Annotations:**
```flux
// Pure function (no effects) - default
fn add(a, b) {
    a + b
}

// Function with IO effect
fn greet(name) with IO {
    print("Hello, #{name}!");
}

// Multiple effects
fn fetch_and_log(url) with IO, Async {
    let data = await http.get(url);
    print("Received: #{data}");
    data
}

// Generic over effects
fn map<A, B, E>(list: List<A>, f: A -> B with E): List<B> with E {
    match list {
        Nil -> Nil;
        Cons(x, xs) -> Cons(f(x), map(xs, f));
    }
}
```

**Effect Inference:**
```flux
// Effects inferred from body
fn process(data) {
    print("Processing...");   // IO effect inferred
    let result = transform(data);
    print("Done!");
    result
}
// Inferred: fn process(data) with IO
```

**Effect Handlers:**
```flux
// Handle effects to provide implementation
handle {
    let data = read_file("config.txt");
    parse(data)
} with {
    IO.read_file(path) -> resume("mock config data");
}

// Useful for testing
fn test_parser() {
    let result = handle {
        parse_config()
    } with {
        IO.read_file(_) -> resume("test=true\nvalue=42");
    };

    assert(result.test == true);
}

// State effect handler
fn run_stateful<S, A>(initial: S, computation: () -> A with State<S>): (A, S) {
    let state = initial;

    let result = handle {
        computation()
    } with {
        State.get() -> resume(state);
        State.set(new_state) -> {
            state = new_state;
            resume(())
        };
    };

    (result, state)
}
```

**Effect Rows (Advanced):**
```flux
// Function that adds IO to existing effects
fn log<E>(msg: String, action: () -> A with E): A with IO, E {
    print(msg);
    action()
}

// Effect subtraction (handler removes effect)
fn pure_random<E>(seed: Int, action: () -> A with Random, E): A with E {
    handle {
        action()
    } with {
        Random.next() -> resume(lcg(seed));
    }
}
```

---

### 3.2 Reactive Streams

Built-in support for reactive programming, fitting the "Flux" name.

#### Stream Type
```flux
// Stream produces values over time
type Stream<T>

// Create streams
let clicks: Stream<Event> = dom.clicks(button);
let ticks: Stream<Int> = Stream.interval(1000);
let values: Stream<Int> = Stream.from([1, 2, 3, 4, 5]);
```

#### Stream Operators
```flux
// Transform
stream
    |> Stream.map(\x -> x * 2)
    |> Stream.filter(\x -> x > 0)
    |> Stream.take(10)
    |> Stream.drop(2)

// Combine
Stream.merge(stream1, stream2)
Stream.zip(streamA, streamB)
Stream.concat(first, second)
Stream.switch(streamOfStreams)

// Time-based
stream
    |> Stream.debounce(300)
    |> Stream.throttle(1000)
    |> Stream.delay(500)
    |> Stream.timeout(5000)

// Accumulate
stream
    |> Stream.scan(0, \acc, x -> acc + x)
    |> Stream.reduce(0, \acc, x -> acc + x)

// Error handling
stream
    |> Stream.catch(\err -> Stream.of(default_value))
    |> Stream.retry(3)
```

#### Stream Syntax
```flux
// Stream literal
stream {
    yield 1;
    yield 2;
    yield 3;
}

// Async stream
stream {
    for url in urls {
        let data = await fetch(url);
        yield data;
    }
}

// Reactive bindings (re-evaluate when dependencies change)
let~ count = 0;
let~ doubled = count * 2;  // automatically updates

// Subscribe
subscribe(stream, \value -> {
    print("Received: #{value}");
});

// Or with |>
stream |> subscribe(\value -> handle(value));
```

#### Reactive State
```flux
// Reactive cell (observable value)
let cell = Cell.new(0);

// Read current value
let current = cell.get();

// Update value
cell.set(42);
cell.update(\x -> x + 1);

// React to changes
cell |> subscribe(\new_value -> {
    render(new_value);
});

// Derived cells
let doubled = cell |> Cell.map(\x -> x * 2);
let combined = Cell.combine(cellA, cellB, \a, b -> a + b);
```

#### Integration with Effects
```flux
// Streams have the Async effect
fn process_stream(s: Stream<Data>) with Async {
    s |> Stream.for_each(\data -> {
        handle(data);
    });
}
```

---

### 3.3 Actor Model

Lightweight concurrent processes with message passing.

#### Actor Definition
```flux
actor Counter {
    // Actor state
    state count = 0;

    // Message handlers
    receive Increment {
        count = count + 1;
    }

    receive Decrement {
        count = count - 1;
    }

    receive GetCount {
        reply(count);
    }

    receive Add(n) {
        count = count + n;
        reply(count);
    }

    // Lifecycle hooks
    on_start {
        print("Counter started");
    }

    on_stop {
        print("Counter stopped");
    }
}
```

#### Message Types
```flux
// Define message types for an actor
messages Counter {
    Increment
    Decrement
    GetCount -> Int
    Add(Int) -> Int
    Reset
}

// Or inline with actor
actor Counter {
    message Increment;
    message GetCount -> Int;

    // ...
}
```

#### Spawning and Communication
```flux
// Spawn an actor
let counter = spawn Counter;
let worker = spawn Worker with { id: 1 };

// Send async message (fire and forget)
send(counter, Increment);
counter ! Increment;  // operator syntax

// Synchronous call (wait for reply)
let value = call(counter, GetCount);
let value = counter ? GetCount;  // operator syntax

// Send with timeout
let result = call(counter, GetCount, timeout: 5000);
match result {
    Ok(value) -> print(value);
    Err(Timeout) -> print("Timed out");
}

// Stop an actor
stop(counter);
```

#### Pattern Matching Messages
```flux
actor Router {
    receive msg {
        match msg {
            { type: "http", method, path, body } -> {
                route_http(method, path, body);
            }

            { type: "ws", data } -> {
                broadcast(data);
            }

            Shutdown -> {
                cleanup();
                stop();
            }

            _ -> {
                print("Unknown message");
            }
        }
    }
}

// Or direct pattern matching
actor Calculator {
    receive Add(a, b) {
        reply(a + b);
    }

    receive Sub(a, b) {
        reply(a - b);
    }

    receive Mul(a, b) {
        reply(a * b);
    }

    receive Div(a, b) {
        if b == 0 {
            reply(Left("Division by zero"));
        } else {
            reply(Right(a / b));
        }
    }
}
```

#### Actor Linking and Supervision
```flux
// Link actors (bidirectional failure notification)
let worker = spawn Worker |> link(self());

// Monitor actor (unidirectional)
let ref = monitor(worker);

// Handle actor death
receive { Down, ref, reason } {
    print("Worker died: #{reason}");
    let new_worker = spawn Worker;
}

// Supervision tree
supervisor AppSupervisor {
    strategy: OneForOne;  // or AllForOne, RestForOne
    max_restarts: 3;
    max_seconds: 60;

    children: [
        { actor: Database, restart: Permanent },
        { actor: Cache, restart: Transient },
        { actor: Logger, restart: Temporary }
    ];
}

// Start supervision tree
let app = spawn AppSupervisor;
```

#### Actor Pools
```flux
// Pool of workers for load balancing
let pool = spawn_pool(Worker, size: 4);

// Send to any available worker
pool ! ProcessJob(data);

// Round-robin distribution
for job in jobs {
    pool ! job;
}
```

#### Actor State Persistence
```flux
actor PersistentCounter {
    state count = 0;

    // Persist state periodically or on change
    persist every: 5000;  // milliseconds

    // Or manual
    receive Increment {
        count = count + 1;
        if count % 100 == 0 {
            persist();
        }
    }

    // Recovery
    on_recover(saved_state) {
        count = saved_state.count;
    }
}
```

---

### 3.4 Unified Example

Combining all features in a realistic application:

```flux
//! User service with reactive updates and supervision

import Flow.Either
import Flow.Stream

// Domain types
type UserId = Int;

type User {
    id: UserId,
    name: String,
    email: String,
    active: Bool
}

type UserEvent {
    Created(User)
    Updated(User)
    Deleted(UserId)
}

// Messages
messages UserService {
    GetUser(UserId) -> Either<String, User>
    CreateUser(name: String, email: String) -> Either<String, User>
    UpdateUser(UserId, updates: Hash) -> Either<String, User>
    DeleteUser(UserId) -> Either<String, ()>
    Subscribe -> Stream<UserEvent>
}

// Actor implementation
actor UserService with IO, State<Hash<UserId, User>> {
    state users = {};
    state next_id = 1;
    state subscribers: List<Stream.Sink<UserEvent>> = [];

    receive GetUser(id) {
        match users[id] {
            Some(user) -> reply(Right(user));
            None -> reply(Left("User not found"));
        }
    }

    receive CreateUser(name, email) {
        // Validate
        if name == "" {
            reply(Left("Name required"));
            return;
        }

        let user = User {
            id: next_id,
            name,
            email,
            active: true
        };

        users = users |> Dict.put(next_id, user);
        next_id = next_id + 1;

        // Notify subscribers
        broadcast(Created(user));

        reply(Right(user));
    }

    receive Subscribe {
        let (stream, sink) = Stream.create();
        subscribers = push(subscribers, sink);
        reply(stream);
    }

    fn broadcast(event: UserEvent) {
        for sink in subscribers {
            sink.emit(event);
        }
    }
}

// Supervisor
supervisor AppSupervisor {
    strategy: OneForOne;

    children: [
        { actor: UserService, restart: Permanent, name: "users" }
    ];
}

// Client code
fn main() with IO, Async {
    // Start supervised system
    let app = spawn AppSupervisor;
    let users = app.child("users");

    // Subscribe to updates
    let events = users ? Subscribe;

    events
        |> Stream.filter(\e -> match e { Created(_) -> true; _ -> false })
        |> subscribe(\e -> print("New user created!"));

    // Create some users
    let alice = users ? CreateUser(name: "Alice", email: "alice@example.com");
    let bob = users ? CreateUser(name: "Bob", email: "bob@example.com");

    // Query
    match alice {
        Right(user) -> {
            print("Created: #{user.name}");

            let fetched = users ? GetUser(user.id);
            match fetched {
                Right(u) -> print("Fetched: #{u.name}");
                Left(err) -> print("Error: #{err}");
            }
        }
        Left(err) -> print("Failed: #{err}");
    }
}
```

---

## Implementation Roadmap

### Phase 1: Essential Syntax (Weeks 1-4)
| Feature | Effort | Priority |
|---------|--------|----------|
| Operators: `<=`, `>=`, `&&`, `\|\|`, `%` | Small | Critical |
| Pipe operator `\|>` | Small | Critical |
| Block comments `/* */` | Small | Medium |
| Lambda shorthand `\x -> expr` | Medium | High |

### Phase 2: Pattern Matching (Weeks 5-8)
| Feature | Effort | Priority |
|---------|--------|----------|
| Pattern guards `if condition` | Medium | High |
| Or-patterns `a \| b` | Medium | Medium |
| Array destructuring `[a, ...rest]` | Medium | High |
| Hash destructuring `{ x, y }` | Medium | High |

### Phase 3: Type System (Weeks 9-14)
| Feature | Effort | Priority |
|---------|--------|----------|
| Tuple type `(a, b)` | Medium | High |
| ADTs / Sum types | Large | High |
| Record types | Large | Medium |
| Type inference improvements | Large | Medium |

### Phase 4: Advanced Features (Weeks 15-24)
| Feature | Effort | Priority |
|---------|--------|----------|
| Default parameters | Small | Medium |
| List comprehensions | Medium | Medium |
| Effect system (basic) | Very Large | High |
| Reactive streams | Large | Medium |
| Actor model | Very Large | Medium |

### Phase 5: Ecosystem (Ongoing)
| Feature | Effort | Priority |
|---------|--------|----------|
| Effect handlers | Large | Medium |
| Supervision trees | Large | Low |
| Tooling (formatter, LSP) | Large | High |
| Package manager | Very Large | Medium |

---

## Open Design Questions

1. **Lambda syntax**: `\x -> expr` vs `|x| expr` vs `fn(x) expr`?

2. **Effect syntax**: `with Effect` vs `!Effect` vs `@Effect`?

3. **Actor syntax**: Keyword `actor` vs `process` vs `agent`?

4. **Stream syntax**: `stream { yield x }` vs generator functions?

5. **Type parameter syntax**: `<T>` vs `[T]` vs `{T}`?

6. **Visibility modifiers**: `pub`/`priv` vs `_prefix` convention vs explicit `export`?

7. **Mutability**: `let mut` vs `var` vs `let!`?

---

## References

### Languages
- **Haskell**: Effect system, ADTs, pattern matching
- **Elixir/Erlang**: Actor model, supervision, streams
- **Rust**: Ownership (not adopted), pattern matching, `|>` alternative
- **Koka**: Algebraic effects, effect handlers
- **Elm**: Simplicity, no runtime exceptions
- **F#**: Pipe operator, computation expressions
- **Scala**: Case classes, pattern guards, for comprehensions

### Papers
- "Programming with Algebraic Effects and Handlers" (Bauer, Pretnar)
- "Concurrent Haskell" (Jones, Gordon, Finne)
- "Making the future safe for the world" (Miller, Armstrong - Actors)

### Existing Implementations
- Koka: github.com/koka-lang/koka
- Unison: github.com/unisonweb/unison
- Eff: github.com/matijapretnar/eff
