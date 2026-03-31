# What's New in Flux v0.0.4

Flux v0.0.4 is a compiler architecture release.

This version replaces the old JIT-centered backend story with a clearer pipeline built around Flux Core, a bytecode VM, and a native LLVM path. It also lands Aether as an operational ownership model across the compiler, expands the standard library, improves diagnostics, and adds the module/cache foundations needed for faster repeated builds.

## Highlights

- **Native backend via LLVM** — compile Flux through Core and LLVM with `--native`
- **Aether is now operational** — compile-time dup/drop insertion, borrowing inference, and reuse/drop specialization
- **Core-first compiler pipeline** — AST lowers into Flux Core, then backend IR, then VM or LLVM
- **Flow standard library expansion** — stdlib and primops are now the main source of language functionality
- **Tail-call and recursion improvements** — guaranteed self-tail-call optimization plus mutual-tail-call support
- **Better multimodule compilation** — `exposing` imports, module interfaces, and cacheable module metadata
- **Diagnostics hardening** — stronger contextual parser/type diagnostics and more stable snapshot coverage

## New Execution Model

### Native backend

Flux now supports a real native compilation path:

```bash
cargo run -- --native examples/basics/fibonacci.flx
```

The current architecture is:

```text
AST -> Core -> cfg -> bytecode -> VM
AST -> Core -> LLVM -> native
```

This release also adds native-oriented code generation work such as:

- typed LLVM lowering through `FluxRep`
- ADTs, pattern matching, closures, and effect handlers on the native path
- AOT support through LLVM IR / binary emission flags
- better native runtime error reporting and stack traces

The old Cranelift JIT path is no longer the center of the compiler. Flux now has one clear interpreted path and one clear native path.

## Aether Memory Model

`v0.0.4` is the release where Aether stops being a design sketch and becomes part of normal compilation.

What landed:

- borrow inference for dup/drop elision
- compile-time dup/drop insertion
- reuse analysis and drop specialization
- operational integration across VM and LLVM code paths
- broader interprocedural ownership precision
- `--dump-aether` for inspecting memory-model decisions

This is also the release that completes the move away from the older GC-shaped runtime paths for lists and ADTs. Runtime ownership is now much more aligned with Flux's Rc-based operational model.

## Language and Module System

### Better modules

Flux modules are more usable in real projects now.

New and improved pieces include:

- `exposing` imports
- module-qualified member access
- better import resolution behavior
- module interface files for cached type/borrow metadata

This release also lays the groundwork for separate compilation and faster repeated runs through interface and cache artifacts.

### Tail calls and recursion

Recursion support improved substantially:

- guaranteed self-tail-call optimization
- trampoline-based mutual tail calls
- CPS continuation stack support for non-tail recursion

This matters directly for functional-style Flux programs, where recursion is a primary control-flow tool.

## Standard Library and Runtime

### Flow standard library

The standard library grew and became more central to the language runtime model.

Highlights:

- Flow stdlib promotion and auto-import behavior
- `Flow.Array`
- more collection and higher-order functions
- more C runtime helpers and primop coverage
- deep structural equality support in the runtime

This release also continues the migration from ad hoc runtime/base helpers toward a clearer primop + Flow stdlib split.

## Diagnostics and Type System

### Diagnostics hardening

`v0.0.4` puts a lot of work into making compiler failures clearer and more stable.

Improvements include:

- better contextual parser recovery
- stronger call-site / let / return diagnostics
- improved runtime error consistency across backends
- deterministic effect-row diagnostics
- more snapshot coverage for parser, compiler, and runtime errors

### Effect rows and HM inference

Type inference also moved forward significantly:

- explicit effect-row tails
- row variables in effect expressions
- effect-row-aware HM unification
- a dedicated constraint solver for effect rows
- more modularized inference code with stable expression IDs

This is mostly compiler-internal work, but users should feel it as better diagnostics and fewer backend inconsistencies.

## Tooling

### New compiler inspection surfaces

This release improves Flux's debugging and compiler-visibility story:

- `--dump-aether`
- richer backend/runtime diagnostics
- better parity-oriented testing and regression coverage
- release/changelog automation scripts

These changes are part of a broader push toward stability and repeatability as the compiler grows more sophisticated.

## Migration Notes

### Backend story changed

If you were using older JIT-oriented workflows, revisit the current CLI and docs. The preferred paths are now:

- VM for fast default execution
- `--native` for native execution and native artifact workflows

### Cache and module artifacts

Flux now emits and consumes more compiler artifacts during repeated builds. These are implementation details, but they may invalidate older caches across compiler upgrades.

## Recommended first things to try

Run a program on both maintained backends:

```bash
cargo run -- examples/basics/arithmetic.flx
cargo run -- --native examples/basics/arithmetic.flx
```

Inspect Aether output:

```bash
cargo run -- --dump-aether examples/basics/arithmetic.flx
```

Try a multimodule example:

```bash
cargo run -- examples/aoc/2024/day06.flx
```

## In short

Flux v0.0.4 is less about new surface syntax and more about making the compiler real:

- one semantic IR
- two maintained execution paths
- an operational ownership model
- stronger modules and caches
- better diagnostics

It is the release that turns Flux from an experimental collection of features into a more coherent compiler platform.
