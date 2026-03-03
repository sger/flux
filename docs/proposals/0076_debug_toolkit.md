- Feature Name: Flux Debug Toolkit
- Start Date: 2026-03-02
- Proposal PR:
- Flux Issue:
- Depends on: 0032 (type system), 0042 (effect rows), 0064 (row variables)

# Proposal 0076: Flux Debug Toolkit

## Summary

Add a graduated three-tier debugging toolkit that covers the full compilation
and execution pipeline without requiring developers to modify Rust source code
for the common cases:

- **Tier 1** — `spy(label, value)` base function: inline value tracing in Flux
  programs, works identically in both VM and JIT backends.
- **Tier 2** — `flux analyze <file.flx>`: a new CLI subcommand that runs the
  pipeline up to and including HM inference, then dumps inferred function
  signatures, effect rows, and HM diagnostics without emitting bytecode or
  executing the program.
- **Tier 3** — `flux analyze --jit <file.flx>`: extends the JIT compiler with
  a Cranelift IR dump and verifier pass, surfaced through the same subcommand.

The three tiers correspond to the three most common bug classes: wrong runtime
values (Tier 1), wrong type/effect inference (Tier 2), and wrong Cranelift IR
generation (Tier 3).

## Motivation

### The debugging gap

Flux has useful existing tools:

- `flux tokens <file.flx>` — lexer output
- `flux bytecode <file.flx>` — disassembled VM bytecode
- `cargo run -- --trace` — VM instruction trace (every opcode)

None of these expose what the type and effect system actually decided. When HM
inference produces an unexpected type, or the effect row solver accepts or
rejects a constraint incorrectly, the only current path is to add `dbg!` or
`eprintln!` inside the Rust compiler source and recompile — a cycle that takes
minutes and requires understanding the Rust internals.

The gap is concrete. For this program:

```flux
fn map_passthrough(xs: Array<Int>, f: (Int) -> Int with |e) -> Array<Int> with |e {
    map(xs, f)
}

fn add_one(n: Int) -> Int { n + 1 }

fn main() -> Unit {
    let doubled = map_passthrough([|1, 2, 3|], add_one)
    let _ = doubled
}
```

If `doubled` has a wrong value, the developer has no tool to answer:

- What did `map_passthrough` actually receive and return?
- Did HM infer `|e` as open or did it collapse to a concrete effect?
- If running with `--jit`, what Cranelift IR was generated for `map_passthrough`?

### Why three tiers

Different bugs live at different layers. Jumping directly to Rust-level
debugging for a value bug wastes time. The tiers exist to intercept bugs at the
cheapest layer possible:

| Bug class | Current path | Toolkit path |
|-----------|-------------|-------------|
| Wrong runtime value | Add `print`, rerun | `spy` in the `.flx` file |
| Wrong inferred type or effect | `dbg!` in Rust, recompile | `flux analyze` |
| Wrong Cranelift IR | `eprintln!` in JIT compiler, recompile | `flux analyze --jit` |
| Rust panic | `RUST_BACKTRACE=full` | Unchanged — still `RUST_BACKTRACE=full` |

### Specific use cases

**1. Pipeline debugging with `spy`**

```flux
fn main() -> Unit with IO {
    let result = [|1, 2, 3|]
        |> spy("input")
        |> map_passthrough(_, add_one)
        |> spy("after map_passthrough")
}
```

```
[spy] input:               [1, 2, 3]   167.flx:13
[spy] after map_passthrough: [2, 3, 4]   167.flx:15
```

**2. Effect row inspection**

When a function is incorrectly rejected for having a mismatched effect row, the
developer runs:

```
$ flux analyze 167_base_hof_callback_effect_row_ok.flx

FUNCTION SIGNATURES
  fn add_one         : (Int) -> Int
  fn map_passthrough : (Array<Int>, (Int) -> Int with |e) -> Array<Int> with |e
  fn main            : () -> Unit

EFFECT ROWS
  add_one         : pure
  map_passthrough : open |e
  main            : pure

HM DIAGNOSTICS
  none
```

No Rust recompile. No `dbg!`. The open-row status of `map_passthrough` is
visible directly.

**3. JIT IR verification**

```
$ flux analyze --jit 167_base_hof_callback_effect_row_ok.flx

CRANELIFT IR: add_one
──────────────────────────────────────
function u0:1(i64, i64, i64) -> i64 system_v {
block0(v0: i64, v1: i64, v2: i64):
    v3 = load.i64 notrap aligned v1+8
    v4 = iconst.i64 1
    v5 = call rt_make_integer(v0, v4)
    v6 = call rt_add_values(v0, v3, v5)
    return v6
}
VERIFIER: ok

CRANELIFT IR: map_passthrough
──────────────────────────────────────
...
```

If the IR for `map_passthrough` is wrong, it is visible without touching a
single Rust file.

## Guide-level explanation

### `spy(label, value)`

`spy` is a new base function with the signature:

```
spy : (String, a) -> a with IO
```

It prints `label` and a formatted representation of `value` to stderr, then
returns `value` unchanged. It has `IO` effect because it writes to stderr.

```flux
-- Basic use
let x = spy("x", compute())

-- Pipeline use (most ergonomic)
let result = fetch_data()
    |> spy("fetched")
    |> transform(_)
    |> spy("transformed")
```

Output format:

```
[spy] fetched:      { id: 1, name: "Alice" }   program.flx:14
[spy] transformed:  { id: 1, name: "alice" }   program.flx:16
```

The file and line are taken from the call site's debug info, matching the
format already used by runtime error messages.

**Effect discipline**: `spy` has `IO` effect. This is intentional — it is
honest about the fact that it writes to stderr. When debugging a pure function,
temporarily annotate it with `with IO` and remove the annotation when the bug
is fixed. This is the same discipline Haskell programmers use with
`Debug.Trace.trace`, without the `unsafePerformIO` escape hatch.

`spy` is **not** available in production build profiles (future: stripped with
`--release` or a `#[debug_only]` annotation). For now it is always available
and it is the developer's responsibility to remove `spy` calls before shipping.

### `flux analyze <file.flx>`

A new CLI subcommand. It runs the pipeline through PASS 1 and HM inference,
then exits — **no PASS 2, no bytecode emission, no VM execution**.

```
$ flux analyze <file.flx> [--strict] [--root <path>]
```

Output sections:

```
FUNCTION SIGNATURES
  fn <name> : <scheme>
  ...

EFFECT ROWS
  <name> : pure | open |<var> | <concrete effects>
  ...

HM DIAGNOSTICS
  [none] | [diagnostic output]
```

If the file fails to parse or PASS 0 validation fails, the subcommand outputs
the diagnostics and exits with code 1, matching the behaviour of the existing
`bytecode` subcommand.

### `flux analyze --jit <file.flx>`

Extends `analyze` to run the JIT compiler through IR generation without
finalising the module or executing code. For each compiled function it outputs:

```
CRANELIFT IR: <function_name>
──────────────────────────────────────
<clif text from ctx.func.display()>

VERIFIER: ok | <error list>
```

The Cranelift verifier (`cranelift_codegen::verify_function`) is always run in
this mode, regardless of build profile. In normal `--jit` execution it is only
run in debug builds.

## Reference-level explanation

### Tier 1: `spy` base function

#### `src/runtime/base/mod.rs`

Add the implementation (appended at the end to preserve stable indices):

```rust
fn base_spy(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    if args.len() != 2 {
        return Err(format!("spy expects 2 arguments, got {}", args.len()));
    }
    let label = match &args[0] {
        Value::String(s) => s.clone(),
        other => return Err(format!("spy: first argument must be String, got {}", other)),
    };
    let value = args[1].clone();
    let formatted = match &value {
        Value::Gc(_) | Value::Tuple(_) | Value::Array(_) => {
            list_ops::format_value(ctx, &value)
        }
        _ => format!("{}", value),
    };
    eprintln!("[spy] {}: {}", label, formatted);
    Ok(value)
}
```

Append to `BASE_FUNCTIONS` slice (index stability is mandatory — new entries
must always be appended, never inserted):

```rust
BaseFunction {
    name: "spy",
    func: base_spy,
    hm_signature: Sig::Spy,
},
```

#### `src/runtime/base/signatures.rs`

Add `Spy` variant to `BaseHmSignatureId` and implement its scheme:

```rust
// In BaseHmSignatureId:
Spy,

// In scheme_for_signature_id():
// spy : (String, a) -> a with IO
BaseHmSignatureId::Spy => {
    let a = TypeVarId::fresh();
    Scheme::new(
        vec![a],
        InferType::Fun(
            vec![InferType::Con(TypeConstructor::String), InferType::Var(a)],
            Box::new(InferType::Var(a)),
            InferEffectRow::concrete(["IO"]),
        ),
    )
}
```

#### Source location in output

`spy` uses `ctx.current_file()` and `ctx.current_span()` (already available on
`RuntimeContext`) to append the call site location to its output. This matches
the format used by `runtime_error_from_string` in `src/runtime/vm/trace.rs`.

#### JIT backend

`spy` is a base function dispatched via `OpCallBase` / `rt_call_base`. No JIT
special-casing is required — the existing base function dispatch path in
`src/jit/runtime_helpers.rs` handles it.

### Tier 2: `flux analyze` subcommand

#### `src/main.rs`

Add `"analyze"` to the subcommand match. The implementation follows the
`show_bytecode` pattern exactly up to the point where PASS 2 would begin, then
diverges to dump rather than emit:

```rust
"analyze" => {
    let use_jit = args.iter().any(|a| a == "--jit");
    let path = /* last positional arg */;
    if use_jit {
        analyze_jit(path, strict_mode, max_errors, diagnostics_format);
    } else {
        analyze_file(path, strict_mode, max_errors, diagnostics_format);
    }
}
```

#### `fn analyze_file()`

```
lex → parse → Compiler::new_with_interner
            → compile PASS 0 (validation)
            → compile PASS 1 (predeclare)
            → infer_program()             ← HM inference
            → STOP — no PASS 2
            → dump type_env (function signatures)
            → dump function_effects (effect rows)
            → dump hm diagnostics
```

`Compiler` needs one small addition — a public accessor for the HM result:

```rust
// src/bytecode/compiler/mod.rs
pub fn hm_snapshot(&self) -> HmSnapshot {
    HmSnapshot {
        type_env:      &self.type_env,
        hm_expr_types: &self.hm_expr_types,
        function_effects: &self.function_effects,
    }
}
```

The output renderer iterates `type_env` bindings alphabetically (deterministic
order), resolves each `Scheme` via `display_infer_type`, and formats effect rows
from `function_effects`.

#### New error code for `analyze` failures

No new error codes are needed. Parse and validation errors already use the
existing diagnostic infrastructure and are rendered identically to other
subcommands.

### Tier 3: `flux analyze --jit`

#### `src/jit/compiler.rs`

Add a `dump_ir: bool` field to `JitCompiler`:

```rust
pub struct JitCompiler {
    pub module: JITModule,
    builder_ctx: FunctionBuilderContext,
    helpers: HelperFuncs,
    dump_ir: bool,          // ← new
    // ...
}
```

In `compile_functions()`, before each `define_function()` call:

```rust
let mut ctx = cranelift_codegen::Context::new();
ctx.func = func;

if self.dump_ir {
    println!("CRANELIFT IR: {}", name_str);
    println!("{}", "─".repeat(40));
    println!("{}", ctx.func.display());

    match cranelift_codegen::verify_function(&ctx.func, &*isa) {
        Ok(_)   => println!("VERIFIER: ok\n"),
        Err(e)  => println!("VERIFIER: {} error(s)\n{:?}\n", e.0.len(), e),
    }
}

self.module.define_function(meta.id, &mut ctx)?;
```

Additionally, in **all debug builds** (`#[cfg(debug_assertions)]`), the
verifier runs silently even without `dump_ir`, and panics with a clear message
if it finds errors. This catches malformed IR in development before it reaches
Cranelift's codegen and produces a cryptic crash.

#### `fn analyze_jit()`

```
lex → parse → JitCompiler::new(dump_ir: true)
            → compile_program()     ← generates CLIF + verifies + prints
            → STOP — no finalize_definitions(), no execution
```

The JIT module is never finalised in `analyze --jit` mode, so no native code is
emitted and no memory is mapped executable.

### Interaction with existing `--trace` flag

`--trace` and `analyze` are independent. `--trace` is a runtime flag that prints
every VM opcode during execution. `analyze` exits before execution. They answer
different questions and are not in conflict.

### Interaction with bytecode cache

`analyze` bypasses the cache (`--no-cache` semantics) because its purpose is to
inspect the live compilation state, not replay a cached result. This is
consistent with the existing `bytecode` subcommand behaviour.

### Snapshot tests

`spy` output goes to **stderr**, not stdout. Existing snapshot tests (which
capture stdout via `examples_fixtures_snapshots`) are not affected. New
snapshot tests for `analyze` output will be added under
`tests/snapshots/analyze/` following the same `insta` pattern.

## Drawbacks

1. **`spy` adds a permanent base function index** — like all base functions, its
   index is stable and cannot be removed without a breaking cache change.
   Future removal would require a deprecation cycle. The function should be
   considered a permanent part of the standard library surface even if
   discouraged in production code.

2. **`spy` has `IO` effect** — developers cannot use it in pure functions
   without temporarily annotating the enclosing function. This is the correct
   behaviour but may be surprising for developers expecting a debug-only escape
   hatch. The alternative (`Debug` effect with special semantics) is more
   complex and deferred to future work.

3. **`analyze` output format is unspecified** — the human-readable text output
   is suitable for interactive debugging but not for tooling. A `--json` mode
   is a natural extension (see Future possibilities) but is out of scope for
   this proposal.

4. **Cranelift verifier in all debug builds** — running the verifier on every
   JIT compilation adds latency in debug builds. Benchmarks should confirm this
   is acceptable (Cranelift's own test suite runs the verifier and considers it
   fast relative to codegen).

## Rationale and alternatives

### Why `spy` and not a `debug` keyword?

A keyword would require parser and AST changes for a feature that should be
removable. A base function is a pure library addition that integrates with the
existing `|>` pipe operator naturally, is searchable in source, and leaves no
compiler surface.

### Why stderr for `spy`?

Stdout is program output. Mixing debug traces into stdout breaks pipelines and
snapshot tests. Stderr is the correct channel for diagnostic output, matching
the convention of all existing error and warning rendering in Flux.

### Why stop `analyze` before PASS 2?

PASS 2 emits bytecode and may trigger additional compiler errors that are
intentionally deferred from PASS 1. Stopping after HM inference gives a clean
view of the type/effect state before any code generation decisions. A developer
investigating a type inference bug does not need to see PASS 2 errors.

### Why not a language server (LSP) instead?

An LSP would eventually supersede Tier 2 (`analyze`) for interactive use. The
CLI subcommand is the right first step because it is testable (snapshot tests),
composable with shell pipelines, and requires no editor integration. The
`analyze` JSON output (future) can serve as the data source for an LSP hover
implementation.

### What is the impact of not doing this?

The debugging cycle for type/effect bugs remains: modify Rust source → `cargo
build` (30–60 seconds) → rerun → interpret raw `dbg!` output → repeat. For JIT
bugs the cycle is identical. This friction slows feature development and makes
the compiler harder to contribute to.

## Prior art

**Haskell `Debug.Trace`** — `trace :: String -> a -> a` is the direct
inspiration for `spy`. The key difference: Haskell's `trace` uses
`unsafePerformIO` to escape the type system. Flux's `spy` uses the effect system
honestly (`with IO`) and is consistent with the language's purity discipline.

**Elm's `Debug.log`** — `Debug.log : String -> a -> a`, writes to the browser
console. Elm strips `Debug.*` calls in optimised builds. The stripping
behaviour is a future possibility for Flux.

**OCaml `Format.eprintf`** — no equivalent of `spy` in the standard library;
debugging is typically done with `Printf.eprintf "%s\n" (show value)`. No
passthrough return value.

**Koka `println`** — effectful, but no single-line passthrough for pipeline
debugging.

**GHC's `-ddump-tc` / `-ddump-types`** — GHC provides many `-ddump-*` flags
for inspecting compiler internals. `flux analyze` is the Flux equivalent,
exposing the HM inference result rather than GHC's full type checker state.

**Cranelift `clif-util`** — Cranelift's own CLI tool can parse and verify CLIF
text files. `flux analyze --jit` takes a different approach: it dumps the CLIF
that the Flux JIT actually generates, in the same compilation context, rather
than requiring the developer to extract and feed it to an external tool.

## Unresolved questions

1. **`spy` stripping in release profiles** — Should `spy` calls be a compile
   error (or warning) in `--release` or `--strict` mode? A `W203: spy_in_release`
   warning would encourage cleanup without blocking. This requires a compile-time
   flag to identify the function as debug-only, which is a cross-cutting concern
   and is left for a follow-up.

2. **`analyze` output format stability** — Is the human-readable text format
   subject to snapshot testing? If yes, changes to `display_infer_type` (e.g.,
   for Proposal 0046 typed AST) would require `analyze` snapshot updates. The
   proposal recommends snapshotting `analyze` output to lock it and detect
   regressions early.

3. **`analyze --jit` without `--features jit`** — Should `flux analyze --jit`
   produce a helpful error message when the JIT feature is not compiled in, or
   silently fall back to the VM `analyze` output? A clear error message is
   preferred.

4. **Per-expression type output** — `analyze` as specified here dumps only
   function-level signatures and effect rows. Dumping per-expression types
   (from `hm_expr_types` + `expr_ptr_to_id`) would require an AST walk to
   resolve spans, as described in Proposal 0046. This is scoped out but should
   be tracked as a Phase 2 enhancement.

5. **Error code range** — No new error codes are introduced by this proposal.
   Confirm before implementation that `W203` (if the `spy` release warning is
   adopted) does not conflict with codes reserved by Proposal 0060.

## Future possibilities

**`flux analyze --json`** — Machine-readable output for tooling and editor
integration. The JSON schema would expose the same data as the text format plus
raw `InferType` values for IDE hover support.

**Language server hover** — Once `analyze --json` exists, a language server can
call `analyze` on save and display inferred types and effect rows on hover,
matching the GHC IDE / rust-analyzer experience.

**`spy` stripping** — In the style of Elm's `Debug` module, emit a compiler
warning (`W203`) when `spy` appears in code compiled with `--strict` or a
future `--release` flag, and provide a `flux lint --fix` autofix that removes
`spy` calls.

**`flux analyze --fn <name>`** — Filter `analyze` output to a single named
function, for programs with many functions where the full dump is too verbose.

**`flux analyze --expr <line>:<col>`** — Show the inferred type of the
expression at a specific source position. Requires the per-expression walk
deferred in the Unresolved questions above.

**`flux diff-analyze`** — Compare `analyze` output between two versions of a
file to understand how a refactor changed the inferred types and effect rows.
Useful for reviewing the impact of type annotation changes.
