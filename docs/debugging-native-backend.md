# Debugging native-backend divergences

This guide describes a repeatable process for investigating cases where the VM
and native LLVM backend disagree. KI-013 is used as the worked example.

## 1. Establish the behavioral difference

Start with the smallest source program that demonstrates the difference.
Record both backends' outputs and whether the native result is:

- a wrong value;
- a runtime error;
- a crash or signal; or
- a timeout or hang.

Run the same input repeatedly. A deterministic failure is easier to reduce and
debug than a sporadic one.

For native Rust integration tests, build and run with one consistent Cargo
invocation. The test crate can have `llvm` enabled while `CARGO_BIN_EXE_flux`
still points to a stale `target/debug/flux` binary built without it:

```text
cargo test --features llvm --test native_json_tests
```

Do not use a preceding plain `cargo build` as the native-binary preparation
step; it can overwrite the shared executable with a non-LLVM build.

For KI-013, the VM returned:

```text
{a={x="1"},b={y={p="2"},z="3"}}
```

while the native backend eventually passed a null element to a tuple-rendering
closure.

## 2. Reduce the input systematically

Remove one feature at a time while preserving the failure. Useful dimensions
include:

- input size and nesting depth;
- pattern shape: constructor, tuple, wildcard, or nested pattern;
- collection shape: empty, singleton, or recursive list;
- ownership-changing operations: reconstruction, mapping, filtering, or reuse;
- backend-only features such as LLVM code generation or native linking.

Keep a minimal reproducer in the test suite once the triggering shape is known.

KI-013 reduced to two table headers, an inline table, and a later key. That
combination forced `Flume.Document.assoc_set` to rebuild a list of tuples.

## 3. Determine where the invalid value first appears

Do not assume that the function where the crash becomes visible caused it.
Trace the value backward through:

```text
consumer -> caller/closure -> collection operation -> producer/reconstruction
```

For native crashes, first identify whether the value is already invalid at the
call boundary. A useful LLDB approach is:

```text
breakpoint set --name <suspected_function>
run
bt
register read
```

For a pointer-like runtime value, inspect the argument register and compare it
with the runtime's sentinel values and minimum valid pointer threshold.

In KI-013, LLDB stopped in the tuple-rendering closure with `x0 = 0`. The call
chain showed that `List.map` supplied the invalid element; the renderer merely
performed the first dereference.

## 4. Inspect generated LLVM and runtime representation

When a value may be either a tagged sentinel or a heap pointer, inspect the
generated LLVM around its use. Verify that the code:

1. checks sentinel values explicitly when the match has sentinel arms;
2. checks the minimum valid pointer threshold before `inttoptr` or loads; and
3. routes invalid values through the language's existing wildcard/default path.

For KI-013, ADT matching already emitted a sentinel/pointer guard, but tuple
matching converted its scrutinee directly with `inttoptr` and dereferenced it.
That was fixed by sharing the same guard behavior with tuple extraction.

## 5. Inspect ownership IR before inspecting the runtime allocator

For reference-counted native backends, dump the ownership-oriented IR and look
for:

- borrowed fields placed into owning constructors without `Dup`;
- fields dropped independently even though they are views into a scrutinee;
- a scrutinee dropped or reused while replacement fields still reference it;
- reuse tokens created from a borrowed or ambiguous provenance; and
- ownership nodes erased while lowering collection constructors.

The important question is whether every value transferred into an owning
constructor has an independent reference. A typical unsafe shape is:

```text
match table with
  Cons((candidate, existing), rest) ->
    MakeTuple(candidate, existing)
```

when `candidate` and `existing` are borrowed views. The safe shape is:

```text
MakeTuple(
  dup candidate in candidate,
  dup existing in existing,
)
```

Borrowed pattern fields should not receive independent `Drop` operations. The
scrutinee owns their storage.

## 6. Use allocator and LLDB evidence for secondary failures

If the crash is detected by `malloc`, do not conclude that allocation caused
it. Set a breakpoint on the allocator entry and inspect the call stack. A
corrupted heap often means an earlier double free, use-after-free, or invalid
reuse.

KI-013's longer manifest reproducer stopped at:

```text
libsystem_malloc.dylib`_xzm_xzone_malloc_freelist_outlined
  -> flux_bump_alloc_slow
  -> flux_Flume_Parse_gather
```

The allocator was only detecting corruption created earlier while parser
results were being gathered and reconstructed.

## 7. Validate in layers

Use progressively broader tests:

1. an ownership/Aether regression for the generated `Dup`/`Drop` shape;
2. an LLVM IR regression for pointer guards;
3. the minimal VM/native reproducer;
4. the affected fixture's VM/native parity test; and
5. adjacent backend and integration test groups.

For KI-013, this sequence covered the Aether reconstruction, tuple guard,
TOML fixture, manifest fixture, and related Flume parity fixtures.

When reviewing `--dump-aether`, distinguish planner output from emitted native
code. `Reuses` and `FBIP` describe the Aether plan. `BorrowedCallGuards` and
the debug locations identify the specific borrowed arguments whose provenance
requires a temporary native `Dup`/`Drop` pair; genuinely linear borrowed calls
remain unguarded and can retain FBIP reuse. This is the provenance-aware fix
for KI-018.

## Practical checklist

- [ ] Reproduce on VM and native backends.
- [ ] Record exact output or signal.
- [ ] Reduce to the smallest source shape.
- [ ] Find the first invalid value, not just the final crash site.
- [ ] Inspect LLDB backtrace and relevant registers.
- [ ] Inspect generated LLVM before pointer conversion/dereference.
- [ ] Dump ownership IR and verify borrowed/owned transitions.
- [ ] Check reuse provenance and replacement-field independence.
- [ ] Add focused regressions before broad parity tests.
- [ ] Document the root cause separately from the symptom.
