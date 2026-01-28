# Debugging & Instrumentation Roadmap

This document outlines pragmatic debugging tooling to make Flux easier to evolve and safer to change.

## 1) Tracing & instrumentation (start here)

### Bytecode execution trace (must-have)
Add a VM trace mode that prints each instruction as it executes.

Example output:

```
IP=0003  OpConstant 0        ; "hello world"
IP=0005  OpCall 1
IP=0007  OpPop
```

What to include:
- instruction pointer (IP)
- opcode name
- operands
- stack snapshot (optional but 🔥)

Enable via:
```
flux run file.flx --trace
```

This alone catches:
- wrong jump offsets
- stack underflows
- bad call arity
- closure capture bugs

### Stack visualization
Show the VM stack after every instruction (or on demand):

```
STACK:
[0] "hello world"
[1] <closure f@3>
```

This is huge for:
- closures
- locals vs free variables
- scope bugs

You already think in bytecode — lean into it.

## 2) Source ↔ bytecode mapping (debug info)

### Instruction → source span mapping
When compiling, attach debug metadata:

```
struct Instruction {
    opcode: OpCode,
    operands: Vec<u16>,
    span: Option<SourceSpan>, // line, column
}
```

Then at runtime:

```
Runtime error at main.flx:12:8
  print(f());
        ^
```

This turns Flux from “VM toy” into a real language.

### Disassembler (flux disasm)
Add a CLI tool:

```
flux disasm file.flx
```

Output:

```
0000 OpConstant 0   ; "hello world"
0003 OpCall 1
0005 OpPop
```

Optionally with source lines:

```
0003 OpCall 1       ; print("hello world")
```

This is invaluable when debugging compiler bugs.

## 3) Assertions & invariants inside the VM

Add debug-only checks (behind `cfg!(debug_assertions)`):
- stack height is valid
- frame base pointer is correct
- instruction pointer is in bounds
- locals index < frame.locals

When one fails:

```
VM invariant violated:
  frame.base_pointer = 3
  stack.len() = 2
```

Fail fast > corrupt silently.

## 4) REPL-powered debugging (Flux superpower)

Your REPL is already a debugger — exploit that.

Step execution:

```
flux> :step
flux> :next
flux> :stack
flux> :locals
```

This gives you:
- interactive stepping
- zero extra UI work
- insanely fast feedback

This is how Smalltalk, Lisp, and early Erlang debuggers felt.

## 5) Structured error objects (not just strings)

Instead of:
```
Err("undefined variable x")
```

Use:
```
RuntimeError {
    kind: UndefinedVariable,
    name: "x",
    span,
    stack_trace,
}
```

Then render nicely:

```
Undefined variable `x`
  at main.flx:7:10
Stack trace:
  main
  f
```

This unlocks:
- better test assertions
- future IDE integration

## 5.1) ICE diagnostics (compiler bugs)

ICE = Internal Compiler Error (a compiler bug, not user code).

Example output:

```
-- INTERNAL COMPILER ERROR -- src/bytecode/compiler.rs -- [ICE002]

unexpected symbol scope for assignment

Hint: src/bytecode/compiler.rs:134 (flux::bytecode::compiler)
```

How to see it:
- Run a compile path and it will print if an invariant is violated:
  ```
  cargo run -- run examples/option_match.flx
  ```
- For a demo, use `examples/ice_demo.flx` and force an ICE temporarily in the compiler.
- If you hit the bytecode cache, remove it first: `rm -rf target/flux`.
- ICEs should be rare; if you see one, it points directly at the Rust file/line.

## 6) Compiler debugging tools (often ignored, very important)

### AST pretty-printer
```
flux ast file.flx --pretty
```

Shows:
- desugared AST
- scopes
- symbol IDs

This is gold for:
- shadowing bugs
- closure capture logic
- let-binding behavior

### Symbol table dump
For each scope:

```
Scope 1 (global):
  print -> Builtin(0)

Scope 2 (function f):
  x -> Local(0)
  y -> Free(0)
```

You’re already halfway there based on your tests.

## 7) Differential testing (advanced but powerful)

Compare Flux behavior against:
- a reference interpreter (slow but correct)
- or a simplified VM

Run the same program on both and assert:
- same output
- same errors

This catches deep semantic bugs.

## 8) Rust-side debugging tools (don’t forget these)

Since Flux is in Rust:
- `dbg!(&value)` inside compiler phases
- `cargo test -- --nocapture`
- `cargo llvm-ir` (for extreme cases)
- `RUST_BACKTRACE=1`

And for VM bugs:
```
RUSTFLAGS="-Zsanitizer=address" cargo run
```
