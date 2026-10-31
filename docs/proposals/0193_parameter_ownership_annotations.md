- Feature Name: `parameter_ownership_annotations`
- Start Date: 2026-10-31
- Proposal PR:
- Flux Issue:
- Status: Draft
- Depends on: [0084](implemented/0084_aether_memory_model.md) (Aether), [0189](0189_aether_owned_moves.md) (owned arguments are moved)
- Relates to: [0130](0130_benchmark_framework.md) (benchmarks), [0192](0192_native_test_cost.md) (scale tests)

# Proposal 0193: Stating a parameter's ownership

## Summary
[summary]: #summary

Aether decides for every function parameter whether the function owns it or
only borrows it, and the programmer has no say. This proposal adds two
parameter annotations, `@own` and `@borrow`, that fix the decision for one
parameter and leave inference in charge of the rest. The first use is Koka's
benchmark pair lookup-borrow and lookup-owned, which measures what borrowing
saves and which Flux cannot express today: its `lookup` is always inferred
borrowed. The annotations also let a library pin a function's calling
convention, so editing a function's body no longer changes what its callers
compile to.

## Motivation
[motivation]: #motivation

### Owned and borrowed parameters

When a function **owns** a parameter, the caller hands over one reference and
the function must release it, or pass it on, before it returns. When a
function **borrows** a parameter, the caller keeps its reference and the
function only reads the value.

Borrowing is cheaper for a function that only reads: the caller does not copy
the reference, and the callee does not release it. Owning is what makes
in-place reuse possible: a function that holds the only reference to a list
cell can rewrite that cell instead of allocating a new one (FBIP).

Flux infers the mode (`src/aether/borrow_infer.rs`). A parameter that is only
read is `Borrowed`; one that is stored, returned, captured or released is
`Owned`. The inferred modes travel with each function into the Aether pass,
the native backend and the module interface (`ModuleInterface::borrow_signatures`).

### The benchmark Flux cannot write

Koka's `test/bench/koka` has two programs that differ in one character:

```koka
fun lookup( ^xs : list<a>, pred : a -> bool) : maybe<a>   // lookup-borrow.kk
fun lookup(  xs : list<a>, pred : a -> bool) : maybe<a>   // lookup-owned.kk
  match xs
    Cons(x,xx) -> if pred(x) then Just(x) else lookup(xx,pred)
    Nil -> Nothing
```

Both search a list of a million numbers for one that is not there, a thousand
times. `^` marks `xs` borrowed. In the owned version every step of the walk
pays for ownership: the caller copies the shared list's reference, and each
cell's match copies the tail and releases the cell. The pair measures that
cost against none at all.

Flux infers the first and has no way to write the second. On 2026-10-31
(`ce4da5da`), `--dump-aether` on a Flux port of `lookup` shows
`aether_call[borrowed, borrowed] lookup(rest, pred)` and no copy or release
of a list cell during the walk. That is the borrowed version. A Flux function
that walks the list the same way but is owned has to do something else with
the list, such as return it, so it is no longer the same program. This was
the one benchmark from the "Speed, Compiled" research that could not be
added with the others (nbody, tak and sfib).

### Callers depend on a function's body

Because the modes are inferred from the body, editing a body can flip a
parameter from borrowed to owned. That changes the module's interface
fingerprint (`InterfaceFingerprintChanged` in `src/types/module_interface.rs`),
so every module that imports it recompiles, and every caller's copies and
releases change. A stdlib function's convention can change by accident in a
refactor that keeps its type. A library author has no way to say "this
parameter is borrowed, keep it that way".

### Checking what inference decided

Bug F2 of proposal 0189 was about 51 stdlib functions inferred to borrow a
parameter they in fact released. The verifier now rejects that
(`BorrowedParamReleased`), but there is still no way to write down the
intended mode and have the compiler compare.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

Most code never needs these annotations: the compiler's choice is usually the
right one. They are for benchmarks, for library functions whose calling
convention should stay fixed, and for tuning code that FBIP should rewrite in
place.

```flux
fn lookup(@borrow xs: List<Int>, pred: (Int) -> Bool) -> Option<Int> {
    match xs {
        [x | rest] -> if pred(x) { Some(x) } else { lookup(rest, pred) },
        [] -> None,
    }
}
```

`@borrow xs` says the caller keeps `xs`. Here inference would choose the same,
so the annotation changes nothing except that it can no longer change.

```flux
fn lookup_owned(@own xs: List<Int>, pred: (Int) -> Bool) -> Option<Int> {
    match xs {
        [x | rest] -> if pred(x) { Some(x) } else { lookup_owned(rest, pred) },
        [] -> None,
    }
}
```

`@own xs` says the function takes the caller's reference. The body is the
same, and the program prints the same result; only the copies and releases
around it differ. `--dump-aether` shows them.

An annotation never makes a program wrong:

- `@own` on a parameter the body only reads makes the function release it at
  its last use. Slower, never unsafe.
- `@borrow` on a parameter the body keeps (stores it in a constructor, returns
  it, passes it to an owned parameter) makes the function copy it at that
  point, as it already does for a field of a borrowed value. Slower, and the
  function can no longer reuse that value in place, but never unsafe.

When an annotation costs something, the compiler says so in a warning, for
example: `@borrow` on `xs`, but `insert` stores it, so it is copied on every
call. Annotations go on the parameters of top-level functions only; on a
lambda's parameters they are an error for now.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The work is in steps, each with its own tests. Steps S3 and S4 change
`src/aether/` and are in a soundness area; each needs `cargo xtask verify`
before it merges.

### S1. Syntax

- `@own` and `@borrow` before a parameter name in `fn` declarations, beside
  the existing function annotations `@fip` and `@fbip`
  (`parse_annotated_function` in `src/syntax/parser/statement.rs`).
- `Statement::Function` gains `parameter_modes: Vec<Option<ParamMode>>`,
  parallel to `parameters` and `parameter_types`, so every pass that walks
  parameters by index keeps working.
- An unknown annotation name, two annotations on one parameter, or an
  annotation on a lambda parameter is a parser error with its own code from
  the registry.
- The formatter and the LSP's signature help print the annotations.

### S2. Into Core

- `CoreDef` gains `declared_modes: Vec<Option<BorrowMode>>`, filled while
  lowering the AST to Core (`src/core/lower_ast/`). Every Core pass that
  rebuilds a definition keeps it, as it keeps `borrow_signature`.
- Dictionary parameters that type classes add in front of a function's own
  parameters get `None`, so the indices stay aligned.

### S3. Borrow inference honours them

- In `infer_borrow_modes_with_preloaded`, a declared mode is a fixed input:
  `ParamConstraint` gains `force_borrowed` beside `force_owned`, and
  `solve_group_modes` never changes a declared parameter.
- Inference still runs for every parameter, declared or not, and records what
  it would have chosen. Where the declared mode costs a copy that inference
  would have avoided, or blocks a reuse, the compiler emits the warning
  described above.
- `BorrowProvenance` gains `Declared`, for a signature with at least one
  declared mode, so dumps and the module interface show where it came from.

### S4. Aether plans a declared borrow that the body consumes

- Inference only marks a parameter borrowed when the body never consumes it,
  so a borrowed parameter that is consumed has not happened until now. The
  planner (`insert_dup_drop_for_def` in `src/aether/insert.rs`) already
  treats borrowed parameters as lent and copies a lent value before an owned
  use (0189's contract, rule 3). This step adds tests that pin that
  behaviour for each way of consuming a value: returning it, storing it in a
  constructor, capturing it in a closure, passing it to an owned parameter,
  and matching on it in a position where the cell would otherwise be reused.
- The debug heap (0188 S10c) checks each test natively: no leak, no double
  release.
- Drop specialisation and reuse must not fire on a declared-borrowed value.
  The verifier's `BorrowedParamReleased` check already rejects a release; a
  test that annotates a function `@fip` and `@borrow` on the parameter it
  would reuse checks that FBIP reports the lost reuse.

### S5. Module interfaces and the cache

- A declared mode is exported in `borrow_signatures` as it is, so callers in
  other modules compile against it.
- The interface fingerprint covers the declared modes. Where every parameter
  of an exported function is declared, the fingerprint no longer depends on
  that function's body, so editing the body does not recompile importers.
- Bump the cache epoch, since cached interfaces gain a field.

### S6. The lookup benchmark pair

- `benchmarks/flux/lookup_borrow.flx` and `lookup_owned.flx`, following
  Koka's: a list of 1,000,000 numbers built once, searched 1,000 times for 0,
  which is not in it; the program prints how many searches found it (0).
- The borrow version annotates `@borrow` rather than relying on inference,
  so a change in inference cannot turn it into the other benchmark.
- Ports where the language can express the difference: Rust (`&[i64]`
  against a `Vec<i64>` or `Rc` list passed by value), Haskell and OCaml have
  no such distinction and get one program run under both names, so the
  table still lines up. Python, Go, Erlang and Elixir likewise.
- Both go in `scripts/bench/bench.sh`'s `extended` group. The README's table
  explains that the pair is about the gap between the two Flux numbers, not
  about the other languages.
- `FLUX_HEAP_STATS=1` on the native debug heap, and `--metrics`
  (`aether_dups`, `aether_drops`), show the difference in counts as well as
  time.

### S7. Documentation

- A short section in `docs/guide/` on ownership: what borrowed and owned mean,
  that the compiler infers them, when an annotation helps, and how to read the
  warning.
- `docs/internals/aether_formal_semantics.md` gains the declared modes.

## Drawbacks
[drawbacks]: #drawbacks

- Two new words to learn, for something most programs should never need. A
  wrong annotation costs speed silently except for the warning.
- S4 exercises a planner path that inference never produced. F2 showed that
  a mismatch between a parameter's mode and its body can be unsound; here the
  planner is meant to copy, and the tests in S4 and the debug heap are what
  stand behind that.
- An exported annotation is a promise. Changing `@borrow` to `@own` later
  changes every caller, as a type change would.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Write the owned benchmark without a language change.** For example,
  `lookup` returns the rest of the list from the match, which forces `xs`
  owned. It works today, but it is a different function from Koka's, so the
  pair would no longer measure only the parameter's mode. It also does
  nothing for interface stability.
- **A compiler flag that turns borrow inference off,** for one function or for
  all. Enough for the benchmark, and less to design, but it cannot be written
  in the source, so it cannot pin a library's convention, and a benchmark
  that needs a flag is easy to run wrongly.
- **Only `@borrow`, as Koka has only `^`.** Koka's parameters are owned unless
  marked; Flux's are inferred, so in Flux "owned" also needs a way to be
  said.
- **Keywords (`own xs`, `borrow xs`) instead of annotations.** They read more
  naturally, but reserve two words or need contextual keywords in the parser.
  `@` already marks annotations that change how a function is compiled
  without changing what it means (`@fip`, `@fbip`), and these are the same
  kind of thing.
- **Rust-style reference types.** A borrowed parameter would have a different
  type, which makes the distinction part of type inference, unification and
  every signature. Far larger, and against Flux's choice that ownership is
  the compiler's business by default.

## Prior art
[prior-art]: #prior-art

- **Koka:** parameters are owned; `^x` marks one borrowed. The
  `lookup-borrow`/`lookup-owned` pair in `test/bench/koka` exists to measure
  it.
- **Lean 4:** borrowing is inferred, and `@&` on a parameter's type marks it
  borrowed explicitly where inference would choose otherwise.
- **Swift:** `borrowing` and `consuming` parameter modifiers (SE-0377) let the
  programmer override the compiler's default convention, with the same
  "never changes meaning, only cost" rule for copyable types.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Whether the cost warning should be on by default or only with a flag, so
   that intentional annotations (a benchmark's `@own`) do not warn. A
   per-annotation way to silence it may be enough.
2. Whether annotations on lambda parameters and on class method declarations
   are needed, and what an annotation on a class method means for its
   instances.
3. Whether the stdlib should annotate its exported functions once this
   lands, which would fix their conventions and remove a source of
   recompiles, and which functions first.

## Future possibilities
[future-possibilities]: #future-possibilities

- A lint that lists exported functions whose inferred modes changed since the
  last release, for library authors who want stability without annotating
  everything.
- `@own` with `@fip` as a checked promise: the function reuses this
  parameter's cells in place, or the build fails.
- Scale tests (0192 S5) on `aether_dups` and `aether_drops` for the lookup
  pair, so a change that makes the borrowed walk copy again is caught by a
  count, not a benchmark.
