# Aether Debugging Guide

This document explains how to read Flux's Aether-shaped Core dumps in
practice.

It complements:

- `docs/internals/aether.md` for the implementation overview
- `docs/internals/aether_formal_semantics.md` for the reduced proof target

This note is operational rather than formal. It is meant to answer questions
like:

- what does `dup` mean in a dump?
- why is there an explicit `drop h` after pattern matching?
- what does `reuse xs ::(..., ...) @mask=0b10` actually mean?
- when should a bug be blamed on Aether versus native lowering/runtime?

---

## 0. Aether Overview

Aether is Flux's ownership-aware transformation pass for Core. Its job is to
make memory management explicit enough that the runtime can use reference
counting and in-place reuse safely.

At a high level, Aether takes ordinary functional Core and enriches it with
operations like:

- `dup x`
- `drop x`
- `reuse token Ctor(...)`
- `drop_spec x { unique ...; shared ... }`

So instead of relying on an invisible GC policy, the program explicitly says:

- when a value is shared
- when a value is consumed
- when a dead value should be released
- when an existing allocation can be recycled

That is why the dump shows `Dups`, `Drops`, `Reuses`, `DropSpecs`, and `FBIP`.

### 0.1 What Problem Aether Solves

Pure functional programs constantly build and discard values:

- match a list node
- use the head
- recurse on the tail
- rebuild a similar list on the way out

If this is done naively, the runtime allocates heavily and can retain values
longer than necessary.

Aether tries to recover the obvious imperative behavior while staying purely
functional in meaning:

- if a value is no longer needed, `drop` it
- if a value is needed in more than one place, `dup` it
- if rebuilding a constructor from an old one, and the old shell is dead,
  `reuse` it instead of allocating again

So semantically it preserves the program, but operationally it gives the
backend a precise ownership plan.

### 0.2 Mental Model

It is useful to think of every Core binder as carrying an ownership mode:

- borrowed: can be observed, not consumed
- owned: this scope is responsible for eventually dropping it
- shared: there are multiple users, so duplication may be needed

Aether computes where values flow and inserts explicit instructions so that
every path balances ownership correctly.

The key rules are:

- every owned pointer-like value should eventually be dropped exactly once
- every extra live use beyond one requires duplication
- reuse is only valid when the old allocation is dead after this point

---

## 1. Where Aether Sits

Flux's pipeline is:

```text
AST
  -> HM type inference
  -> Core lowering
  -> Core passes
  -> Aether shaping / verification
  -> backend lowering
```

Aether is not a separate production IR. It is a set of ownership-aware Core
transforms that make memory-management behavior explicit before VM or native
lowering.

The important consequence is:

- if `--dump-core=debug` already shows nonsense ownership, suspect Aether/Core
- if `--dump-core=debug` looks coherent but the backend misbehaves, suspect
  lowering/runtime

---

## 2. What Aether Makes Explicit

Aether turns implicit ownership behavior into explicit Core operations:

- `dup x`
- `drop x`
- `reuse token Ctor(...)`
- `drop_spec x { ... }`
- `aether_call[...] f(...)`

These are the main operational ideas.

### 2.1 `dup`

`dup x` means the value must survive along more than one live path, so the
compiler materializes sharing explicitly.

Operationally:

- pointer-like values: increment RC / preserve shared ownership
- immediates: no-op in practice

Example:

```text
let y = x
f(x, y)
```

If both `x` and `y` survive independently, one of those uses must come from a
`dup`.

In the `sort_by_string_len` dump:

```text
let %t284 = aether_call[owned] length#238(dup xs#457
```

That means `merge_sort_do` still needs `xs` after calling `length(xs)`, so it
duplicates `xs` before passing it to a consuming call.

### 2.2 `drop`

`drop x` means ownership of `x` ends at that point.

Operationally:

- pointer-like values: decrement RC and maybe free recursively
- immediates: no-op in practice

The key invariant is:

- if Aether emits `drop x`, then `x` is dead after that point

Example from the dump:

```text
drop h#243
```

inside `length_go`.

That means after matching `[h | t]`, the head is not needed anymore, so Aether
releases it before recurring on `t`.

### 2.3 `reuse`

`reuse token Ctor(...)` means the old constructor shell represented by `token`
is dead and may be recycled in place if uniqueness permits.

This is how Flux expresses Perceus-style constructor reuse.

For lists:

```text
reuse xs ::(head, tail)
```

means "rebuild a cons cell, potentially using the old `xs` allocation".

Example from the dump:

```text
reuse left#463 ::(a#466, #4000850[synthetic]#850) @mask=0b10
```

This means:

- `left` is an old cons cell
- build a new cons cell from it
- keep some fields unchanged
- rewrite only the changed ones

### 2.4 `field_mask`

Some reuse sites have a mask:

```text
reuse left ::(a, new_tail) @mask=0b10
```

For a two-field cons cell:

- bit 0 = head
- bit 1 = tail

So `0b10` means:

- tail changed and must be rewritten
- head is unchanged and may stay in place during true in-place reuse

This optimization is only valid when the shell is actually reused. It must not
be applied to a fresh zeroed allocation.

### 2.5 `drop_spec`

`drop_spec` is drop specialization.

It lets the compiler distinguish:

- unique fast path: fields can be moved/reused aggressively
- shared path: fields must be preserved more conservatively

This is a Core-level ownership optimization, not just a backend trick.

### 2.6 `aether_call[...]`

`aether_call[...]` makes argument ownership modes explicit at the call site.

Examples:

- `aether_call[owned, owned] f(x, y)`
- `aether_call[owned, borrowed] f(x, y)`

These annotations tell you whether the callee conceptually consumes an argument
or only borrows it. That drives where `dup` and `drop` must appear around the
call.

Examples from the dump:

```text
aether_call[owned, owned] length_go#240(t#244, ...)
aether_call[owned, borrowed] merge_sort_by#452(xs#220, f#221)
```

---

## 3. How To Read A Dump

The most useful command is:

```bash
cargo run -- --dump-core=debug examples/repros/sort_by_string_len.flx
```

For a filtered view:

```bash
cargo run -- --dump-core=debug examples/repros/sort_by_string_len.flx \
  | rg 'DropSpecialized|Reuse|drop|reuse|merge_sort|merge_by_key|length'
```

When reading a dump:

1. find the relevant function
2. look for `dup`, `drop`, `reuse`, `drop_spec`
3. ask whether the ownership story is coherent
4. only then compare with VM/native behavior

The summary line is helpful:

```text
Dups: 18  Drops: 57  Reuses: 14  DropSpecs: 3  FBIP: fbip(30)
```

but for debugging, the local placement of individual `dup`/`drop`/`reuse`
sites matters much more than the totals.

The core question is:

- does the dump tell a coherent ownership story?

If yes, the bug is usually in lowering/runtime. If not, Aether/Core is a
better suspect.

---

## 4. Case Study: `sort_by_string_len`

The repro is:

- `examples/repros/sort_by_string_len.flx`

It sorts:

```text
["bb", "a", "ccc"]
```

by `string_len`.

The relevant library functions are in:

- `lib/Flow/List.flx`

especially:

- `length`
- `merge_sort_by`
- `merge_sort_do`
- `merge_by_key`

Before the native fixes, this repro produced bad native outputs such as:

- `"[<value>, <value>, <value>]"`
- `"[0, 0, "ccc"]"`

VM output was correct. That already suggested the bug was downstream of Aether.

### 4.1 Relevant Dump Fragment: `length`

From `--dump-core=debug`:

```text
letrec length =
    letrec length_go#240 = (λys#241, acc#242.
          drop h#243
          aether_call[owned, owned] length_go#240(t#244, #4000734[synthetic]#734)
    aether_call[owned, owned] length_go#240(xs#239, 0)
```

This is exactly what Aether should be doing for:

```flux
match ys {
  [h | t] -> length_go(t, acc + 1),
  _ -> acc
}
```

The ownership story is:

- destructure `ys` into `h` and `t`
- `h` is not needed after the match arm begins
- `t` survives into the recursive call
- therefore `drop h` is correct

This is not evidence of an Aether bug. It is evidence that Aether expects
pattern-bound fields to be materialized as owned binders.

That expectation was exactly what the native backend violated before the fix:

- it extracted `h` and `t` from the cons cell
- but did not retain them first
- then it executed the Aether-directed `drop h`
- which freed the head string too early

So this dump line was correct. The native implementation of it was wrong.

This is how Aether sees pattern matching in general.

For:

```flx
match ys {
  [h | t] -> length_go(t, acc + 1),
  _ -> acc
}
```

Aether conceptually reasons:

```text
extract h, t from ys
drop h
call length_go(t, acc + 1)
```

This is valid if extraction gives owned bindings.

### 4.2 Relevant Dump Fragment: `merge_sort_do`

From the dump:

```text
let %t284 = aether_call[owned] length#238(dup xs#457
```

This is also reassuring.

`merge_sort_do` needs `xs` for both:

- `length(xs)`
- `split_at(xs, mid)`

So Aether duplicates `xs` before the consuming call to `length`.

That is exactly the kind of ownership balancing we want to see.

### 4.3 Relevant Dump Fragment: `merge_by_key`

From the dump:

```text
letrec merge_by_key =
                let %t290 = aether_call[owned, owned, borrowed] merge_by_key#462(at#467, right#464, key_fn#465)
                reuse left#463 ::(a#466, #4000850[synthetic]#850) @mask=0b10
                let %t291 = aether_call[owned, owned, borrowed] merge_by_key#462(left#463, bt#469, key_fn#465)
                reuse right#464 ::(b#468, #4000851[synthetic]#851) @mask=0b10
            drop right#464
        drop left#463
```

This is the ownership/reuse story for:

```flux
if key_fn(a) <= key_fn(b) {
  [a | merge_by_key(at, right, key_fn)]
} else {
  [b | merge_by_key(left, bt, key_fn)]
}
```

In the first branch:

- `left` has been consumed by pattern matching
- the new result is a cons cell
- the head `a` stays the same
- the tail changes to the recursive result

So:

```text
reuse left ::(a, new_tail) @mask=0b10
```

is exactly the right Aether shape.

This is also how Aether sees recursive list rebuilds in general.

For:

```flx
[a | merge_by_key(at, right, key_fn)]
```

Aether notices:

- the old `left` shell is consumed by the match
- the new result is another cons cell
- the head `a` is unchanged
- the tail is new

So instead of always allocating:

```text
alloc cons(a, new_tail)
```

it can emit:

```text
reuse left ::(a, new_tail) @mask=0b10
```

That tells the backend:

- if `left` is uniquely owned, recycle its shell
- only rewrite the tail field
- leave the head field alone

Again, this was not the bug.

The native bug was that the lowering treated:

- true in-place reuse
- fresh fallback allocation

as if they were the same case. It then applied the selective-write mask to a
fresh zeroed allocation. That left skipped fields as zero and caused native
results like:

```text
[0, 0, "ccc"]
```

So the dump once again showed a valid Aether intent, but the backend violated
it.

---

## 5. Why The `sort_by_string_len` Bug Was Not An Aether Bug

The two critical dump sites were:

```text
drop h#243
```

and:

```text
reuse left#463 ::(a#466, ...) @mask=0b10
```

Both are semantically reasonable:

- `drop h#243` says the matched head is dead in `length_go`
- `reuse left#463 ... @mask=0b10` says rebuild the consumed cons shell while
  preserving the unchanged head

The actual bugs were in native lowering:

1. extracted pattern fields were not retained before later `drop`
2. masked reuse writes were incorrectly applied to fresh fallback allocations
3. reused ADT headers were written as tagged values instead of raw `i32`

That is why the dump was coherent while native execution was wrong.

This distinction matters:

- if the dump had shown nonsensical ownership, Aether would be suspect
- here the dump showed sensible ownership, so the backend was suspect

### 5.1 How Aether Likely Works Internally

At a high level, the pass behaves roughly like this:

1. Traverse Core expressions.
2. Track usage counts, liveness, and tail positions.
3. Identify which binders are:
   - dead after a point
   - used multiple times
   - consumed by constructors or calls
4. Insert:
   - `dup` before shared consumption
   - `drop` at last use
   - `reuse` when a dead constructor shell is rebuilt
   - `drop_spec` where uniqueness can split optimized and shared paths
5. Produce Aether-shaped Core that later lowerers must implement faithfully.

This is similar in spirit to Perceus/Koka-style reference-count-aware
compilation.

#### Simple Example

Suppose:

```flx
fn f(xs) {
  match xs {
    [h | t] -> [h | g(t)],
    _ -> []
  }
}
```

Aether may reason:

- `xs` is consumed by the match
- `h` survives into the result
- `t` is consumed by `g`
- the old cons shell can maybe be reused

So conceptually it might become:

```text
match xs:
  cons(h, t):
    new_tail = call g(t)
    reuse xs ::(h, new_tail) @mask=0b10
  nil:
    []
```

If `h` were not preserved, it could instead emit `drop h`.

---

## 6. Practical Heuristics

When debugging Aether-related behavior, use these rules.

### 6.1 Suspect Aether when

- `--dump-core=debug` shows obviously impossible ownership
- a value is dropped before a visible later use in the same Core path
- reuse appears for a value that is clearly still live elsewhere
- call ownership modes are inconsistent with later uses

### 6.2 Suspect lowering/runtime when

- `--dump-core=debug` looks coherent
- VM behavior is correct
- native-only output is wrong
- object corruption or use-after-free appears after a sensible `drop`/`reuse`
  story in Core

### 6.3 Good debugging sequence

1. inspect the source fixture
2. inspect `--dump-core=debug`
3. identify the exact `dup`/`drop`/`reuse` sites involved
4. inspect emitted backend IR / LLVM only after Core ownership looks correct
5. instrument runtime only after the ownership contract is understood

That order avoids blaming Aether for backend bugs.

---

## 7. Short Summary

Aether is Flux's ownership-shaping layer on Core. It makes explicit:

- who owns a value
- when a value is shared
- when a value dies
- when a dead constructor shell can be reused

The `sort_by_string_len` bug is a good example of how to read dumps correctly:

- the Aether dump showed a coherent ownership program
- the native backend misimplemented that program
- the fix belonged in LIR/LLVM lowering, not in Aether itself

If you are worried that "Aether is doing something strange", the right first
question is:

- "does the Aether dump tell a coherent ownership story?"

For this bug, the answer was yes.
