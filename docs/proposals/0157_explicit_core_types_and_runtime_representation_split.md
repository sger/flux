- Feature Name: Explicit Core Types and Runtime Representation Split
- Start Date: 2026-04-15
- Status: Draft
- Proposal PR:
- Flux Issue:
- Depends on: 0104 (Flux Core IR), 0119 (Typed LLVM Codegen), 0134 (Shared Low-Level IR), 0153 (Decouple Aether from Core IR), 0156 (Static Typing Completion Roadmap)

# Proposal 0157: Explicit Core Types and Runtime Representation Split

## Summary
[summary]: #summary

Remove `CoreType::Dynamic` and `IrType::Dynamic` as semantic placeholders from the maintained compiler path by separating two concerns that are currently conflated:

1. semantic typing in `core` and maintained lowering
2. runtime representation and calling convention in `aether` / `cfg` / native backends

The proposal introduces explicit polymorphic and unknown semantic type forms in Core, keeps runtime representation in `FluxRep` and backend-specific lowering metadata, and narrows CFG typing so generic boxed values are represented as runtime representation, not as a fake semantic type.

This proposal does **not** reintroduce `Any`, does **not** add a second semantic IR, and does **not** collapse `core`, `aether`, and `cfg` into one layer. It makes the existing pipeline more principled:

```text
AST/HM semantic types
  -> Core semantic types
  -> Aether ownership/reuse lowering
  -> CFG/native runtime representation lowering
```

## Motivation
[motivation]: #motivation

Proposal `0156` closed the front-end static-typing gap:

- source annotations no longer rely on `Any`
- HM inference no longer uses `Any` as a maintained-path fallback
- runtime boundary lowering no longer reintroduces `Any`
- strict typing is semantic in maintained paths

However, the maintained downstream pipeline still has an architectural blur:

- `CoreType::Dynamic` is used where the compiler means “semantic type is polymorphic, abstract, or not yet lowered concretely”
- `IrType::Dynamic` is used where the compiler means “this value is represented generically at runtime”

Those are different ideas.

### Why this matters

Today, `Dynamic` is carrying at least four distinct meanings:

1. **HM unresolved residue**
   - an inference variable or unsupported high-level shape reached the Core boundary

2. **Polymorphic semantic value**
   - a type-class method, dictionary argument, or closure capture is semantically well-typed but not monomorphic

3. **Backend-generic runtime representation**
   - code generation will treat this value as tagged/boxed/generic at runtime

4. **Missing type plumbing**
   - join block parameters, handler continuations, and closure captures do not currently carry explicit semantic types

This conflation has three concrete costs:

1. **Core IR is weaker than it should be**
   - `core` is supposed to be the canonical semantic IR
   - a semantic IR should model semantic unknowns and polymorphism explicitly, not via a generic catch-all node

2. **Backend lowering is harder to reason about**
   - `cfg` cannot tell whether `Dynamic` means “boxed runtime value” or “we lost semantic information upstream”
   - this blurs the boundary between correctness and representation

3. **Proposal `0156` cannot be closed with a strong architectural claim**
   - front-end static typing is complete
   - but the downstream story still uses `Dynamic` in places where other compilers would keep semantic type and runtime representation separate

### Current evidence in Flux

Representative remaining sites:

- `CoreType::Dynamic` in [src/core/mod.rs](/Users/s.gerokostas/Downloads/Github/flux/src/core/mod.rs:99)
- `IrType::Dynamic` in [src/cfg/mod.rs](/Users/s.gerokostas/Downloads/Github/flux/src/cfg/mod.rs:274)
- function result fallback in [src/core/to_ir/fn_ctx.rs](/Users/s.gerokostas/Downloads/Github/flux/src/core/to_ir/fn_ctx.rs:159)
- case join parameters in [src/core/to_ir/case.rs](/Users/s.gerokostas/Downloads/Github/flux/src/core/to_ir/case.rs:28)
- closure captures and handler parameters in [src/core/to_ir/closure.rs](/Users/s.gerokostas/Downloads/Github/flux/src/core/to_ir/closure.rs:207)

These are no longer “gradual typing” in the `Any` sense, but they are still signs that semantic typing and runtime representation have not been fully separated.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

Compiler contributors should think about this proposal in one sentence:

> `Dynamic` should stop meaning “semantic type we do not want to model.”

After this proposal:

- `core` remains the only semantic IR
- `core` can represent polymorphism, abstract values, and unknown-yet-lowered shapes explicitly
- `aether` remains the ownership/reuse layer, not a semantic type fixer
- `cfg` and native lowering operate on runtime representation choices, not on erased semantic placeholders

### The mental model

There are three different questions:

1. **What is the semantic type of this value?**
   - examples: `Int`, `List<a>`, `forall a. a -> a`, `Eq<a> dictionary`, `t0`

2. **Is that semantic type fully known and monomorphic here?**
   - maybe yes, maybe no

3. **How is this value represented at runtime?**
   - examples: unboxed int, boxed heap object, tagged generic value, closure pointer

Flux should answer those with different data structures, not with one `Dynamic`.

### Contributor-facing examples

#### Example 1: polymorphic class method

Today, a dumped Core method can look like:

```text
letrec eq : (Dynamic, Dynamic) -> Bool = ...
```

After this proposal, the semantic Core type should stay explicit, for example as a polymorphic function shape or a Core binder carrying quantified/type-variable structure. The backend may still lower the runtime calling convention generically, but the semantic IR should not pretend the type is “Dynamic”.

#### Example 2: case join parameter

Today, join blocks often get:

```text
IrBlockParam { ty: IrType::Dynamic, ... }
```

After this proposal, the join should carry:

- either the actual semantic type if known
- or an explicit “abstract semantic type variable” form

and the backend should separately decide whether that becomes a tagged generic runtime value.

#### Example 3: closure capture

Today, closure captures are typed as `IrType::Dynamic` because the CFG IR does not distinguish:

- semantic capture type
- closure environment runtime slot representation

After this proposal, closure environments should be modeled as:

- semantically typed captured binders
- lowered runtime slots chosen later by backend lowering

### What this proposal does not mean

- It does not mean “every runtime value must be monomorphic and unboxed”.
- It does not mean “delete generic boxed runtime values”.
- It does not mean “replace `Dynamic` with a different spelling.”

It means:

- semantic unknowns/polymorphism must be explicit as semantic forms
- runtime genericity must be explicit as representation forms

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Design overview

The proposal has four implementation tracks:

1. add explicit semantic non-concrete forms to Core
2. thread semantic types through Core constructs that currently drop them
3. narrow CFG typing so it models runtime representation instead of semantic fallback
4. update dumps, validation, and tests to reflect the new split

---

### Track 1 — Replace `CoreType::Dynamic` with explicit semantic forms

`CoreType` should no longer include:

- `Dynamic`

Instead, introduce explicit forms such as:

```rust
pub enum CoreType {
    Int,
    Float,
    Bool,
    String,
    Unit,
    Never,
    List(Box<CoreType>),
    Array(Box<CoreType>),
    Tuple(Vec<CoreType>),
    Function(Vec<CoreType>, Box<CoreType>),
    Option(Box<CoreType>),
    Either(Box<CoreType>, Box<CoreType>),
    Map(Box<CoreType>, Box<CoreType>),
    Adt(Identifier, Vec<CoreType>),
    Var(CoreTypeVarId),
    Forall(Vec<CoreTypeVarId>, Box<CoreType>),
    Abstract(CoreAbstractTypeId),
}
```

The exact shape can vary, but the required properties are:

- semantic type variables are representable
- polymorphic function types are representable
- abstract type placeholders are explicit and distinct from runtime representation

#### Required supporting changes

- `CoreType::try_from_infer` should produce:
  - concrete `CoreType` where possible
  - `CoreType::Var(...)` for remaining inference variables that survive to Core
  - no `Dynamic` fallback
- `CoreDef.result_ty` and related Core metadata should use these explicit forms
- `FluxRep::from_core_type` should no longer inspect a fake semantic `Dynamic`
  - generic/abstract semantic forms should map to `TaggedRep` or boxed representation through a dedicated representation rule

---

### Track 2 — Add semantic type plumbing to Core constructs

Several maintained-path nodes currently lose type information because the IR does not carry it.

#### 2a. Lambda parameters and closure captures

`CoreExpr::Lam` currently stores only binders:

```rust
Lam { params: Vec<CoreBinder>, body, span }
```

This should become typed:

```rust
Lam {
    params: Vec<CoreParam>,
    result_type: Option<CoreType>,
    body,
    span,
}
```

where `CoreParam` carries:

- binder identity
- semantic parameter type
- optionally already-known runtime rep

Closure lowering should then use these typed params instead of defaulting captured and lambda-introduced values to `IrType::Dynamic`.

#### 2b. Handler parameters and resumes

`CoreHandler` and Aether handler lowering should carry:

- semantic parameter types for operation arguments
- semantic type for the resume function

This removes the need for handler-arm closure parameters to default to generic `Dynamic`.

#### 2c. Case joins and continuation parameters

Case lowering and handle-scope lowering currently invent untyped join parameters.

Instead, join points should carry an explicit semantic join type:

- arm result type if known
- explicit Core type variable/abstract type otherwise

This can be modeled either:

- directly in Core before lowering to CFG, or
- in typed lowering metadata consumed by Core→CFG lowering

---

### Track 3 — Split semantic type from runtime representation in CFG

`IrType` currently mixes semantic and representation intent. It should move toward runtime representation typing.

Two viable designs are acceptable.

#### Option A — Keep `IrType`, redefine it as runtime representation only

Example:

```rust
pub enum IrType {
    Int,
    Float,
    Bool,
    String,
    Unit,
    Never,
    Boxed,
    Tagged,
    FunctionPtr(usize),
    Tuple(usize),
    List,
    Array,
    Hash,
    Adt(AdtId),
}
```

Then:

- old `IrType::Dynamic` becomes either `Tagged` or `Boxed`, depending on runtime representation
- semantic typing remains in Core metadata and per-var/block metadata

#### Option B — Keep semantic type metadata alongside rep-oriented `IrType`

Example:

- `IrType` becomes rep-oriented
- `IrVar`, `IrParam`, and `IrBlockParam` also carry `Option<CoreType>` or equivalent semantic metadata

This is heavier but often easier for debugging and future optimization.

#### Recommendation

Prefer **Option A** if:

- CFG is intended primarily as backend/runtime IR

Prefer **Option B** if:

- parity debugging and future typed optimization want semantic type visibility inside CFG

Either way, the required invariant is:

> `IrType::Dynamic` must not remain as a semantic placeholder.

---

### Track 4 — Validation, dumps, and migration

#### Validation

Add or strengthen:

- Core lint:
  - no `Dynamic` semantic type remains
  - lambdas/handlers/joins have typed parameter/result metadata
- CFG lint:
  - no semantic placeholder type remains in IR typing
  - runtime representation typing is internally consistent

#### Dumps

`--dump-core` should show:

- explicit type variables / polymorphic forms
- no synthetic `Dynamic` for class methods and dictionaries

`--dump-aether` and backend dumps should show:

- runtime representation decisions explicitly
- generic tagged/boxed runtime values by representation name, not semantic `Dynamic`

#### Migration strategy

Phase the implementation in this order:

1. remove `CoreType::Dynamic` by introducing explicit Core semantic forms
2. thread typed params/results through lambdas, handlers, and joins
3. replace `IrType::Dynamic` with rep-oriented forms
4. update snapshots and debugging surfaces

---

### Concrete implementation slices

#### Slice 1 — Core type expressivity

Files:

- [src/core/mod.rs](/Users/s.gerokostas/Downloads/Github/flux/src/core/mod.rs:1)
- [src/core/display.rs](/Users/s.gerokostas/Downloads/Github/flux/src/core/display.rs:1)
- [src/core/lower_ast/mod.rs](/Users/s.gerokostas/Downloads/Github/flux/src/core/lower_ast/mod.rs:1)

Deliverables:

- remove `CoreType::Dynamic`
- add explicit semantic variable/forall/abstract forms
- update `try_from_infer` and Core formatting

#### Slice 2 — Typed Core binders/params

Files:

- `src/core/mod.rs`
- `src/core/lower_ast/*`
- `src/core/passes/*`
- `src/core/to_ir/*`

Deliverables:

- typed lambda params
- typed handler params/resume
- typed join result metadata

#### Slice 3 — Rep-oriented CFG typing

Files:

- [src/cfg/mod.rs](/Users/s.gerokostas/Downloads/Github/flux/src/cfg/mod.rs:1)
- [src/core/to_ir/mod.rs](/Users/s.gerokostas/Downloads/Github/flux/src/core/to_ir/mod.rs:1)
- [src/core/to_ir/fn_ctx.rs](/Users/s.gerokostas/Downloads/Github/flux/src/core/to_ir/fn_ctx.rs:1)
- [src/core/to_ir/case.rs](/Users/s.gerokostas/Downloads/Github/flux/src/core/to_ir/case.rs:1)
- [src/core/to_ir/closure.rs](/Users/s.gerokostas/Downloads/Github/flux/src/core/to_ir/closure.rs:1)

Deliverables:

- replace `IrType::Dynamic`
- carry `Tagged`/`Boxed`/other rep-oriented forms or semantic metadata as chosen
- update CFG validation

#### Slice 4 — Snapshot/test updates

Files:

- `tests/ir_pipeline_tests.rs`
- `tests/aether_cli_snapshots.rs`
- `tests/snapshots/aether/*`

Deliverables:

- class-method and dictionary snapshots stop printing `(Dynamic, Dynamic) -> ...`
- closure/join/handler regression tests assert typed or rep-oriented output

## Drawbacks
[drawbacks]: #drawbacks

- This is a larger architectural change than a local cleanup pass.
- It touches `core`, `aether`, `cfg`, debug output, and snapshot baselines together.
- If done poorly, it could introduce a pseudo-second semantic layer inside CFG.
- The transition may temporarily increase implementation complexity before the model becomes simpler.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why this design

Because Flux already has the right top-level architecture:

```text
Core = semantic IR
Aether = ownership/reuse IR
CFG/LIR = backend/runtime IR
```

What is missing is not another stage, but a cleaner boundary between:

- semantic type information
- runtime representation information

This proposal fixes that at the type-model level.

### Why not keep `Dynamic` and just rename it

Because the problem is semantic, not cosmetic. A renamed `Dynamic` would still conflate:

- polymorphism
- abstract semantic type
- runtime genericity
- missing type plumbing

### Why not leave Core as-is and only tighten CFG

Because `CoreType::Dynamic` is already evidence that semantic information is being erased too early. Fixing only CFG would preserve the blur in the canonical semantic IR.

### Why not put full HM `InferType` directly into Core

That is possible in theory, but it would likely over-couple Core to HM-specific machinery:

- effect-row details
- inference-variable structure
- higher-kinded application details

The better design is to define a Core-owned explicit semantic type model that can represent the needed concepts without importing all of HM internals.

### Why not add a second typed semantic IR

Because `core` is already the semantic IR and should remain so. Proposal `0104` and the current compiler architecture are correct on that point.

## Prior art
[prior-art]: #prior-art

### GHC

Relevant local sources:

- [compiler/GHC/Builtin/Types.hs](/Users/s.gerokostas/Downloads/Github/ghc/compiler/GHC/Builtin/Types.hs:2026)
- [compiler/GHC/Tc/Utils/TcType.hs](/Users/s.gerokostas/Downloads/Github/ghc/compiler/GHC/Tc/Utils/TcType.hs:348)
- [compiler/GHC/Tc/Utils/TcMType.hs](/Users/s.gerokostas/Downloads/Github/ghc/compiler/GHC/Tc/Utils/TcMType.hs:1856)

Relevant lesson:

- GHC keeps semantic typing explicit in Core and related typed compiler structures.
- Runtime representation is modeled separately with `RuntimeRep`, `Levity`, and `PrimRep`.
- When code generation needs a fixed runtime rep, GHC checks or defaults runtime-representation variables explicitly rather than erasing semantic typing into a fake dynamic type.

This is the clearest precedent for Flux:

> typed semantic IR, separate runtime-representation reasoning

### Koka

Relevant local sources:

- [src/Core/Core.hs](/Users/s.gerokostas/Downloads/Github/koka/src/Core/Core.hs:1)
- [src/Type/Type.hs](/Users/s.gerokostas/Downloads/Github/koka/src/Type/Type.hs:90)
- [src/Backend/C/Box.hs](/Users/s.gerokostas/Downloads/Github/koka/src/Backend/C/Box.hs:43)
- [src/Kind/Repr.hs](/Users/s.gerokostas/Downloads/Github/koka/src/Kind/Repr.hs:112)

Relevant lesson:

- Koka’s Core carries explicit semantic `Type` structure, including type variables and quantified types.
- Backend representation is handled through explicit value/data representation analysis and a dedicated boxing/unboxing phase.
- Koka does not solve this by weakening Core typing; it solves it by making representation a later and separate pass.

This is especially relevant for Flux because it mirrors the intended `core -> aether -> backend` layering:

> keep semantic types in Core, handle representation later and explicitly

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- What is the smallest Core-owned type model that can replace `Dynamic` without importing full HM internals?
- Should CFG carry semantic type metadata alongside rep-oriented `IrType`, or should semantic typing stop at Core/Aether?
- Should closure captures be typed semantically, by runtime rep, or both?
- How should polymorphic class-method and dictionary Core dumps be rendered once `Dynamic` is gone?
- Which parts of the older non-canonical AST→CFG lowering path should be updated versus explicitly left legacy?

## Future possibilities
[future-possibilities]: #future-possibilities

- Cleaner typed Core opens the door to more principled Core-level optimization without backend-type ambiguity.
- Explicit rep-oriented CFG typing can improve VM/native parity debugging because representation decisions become visible and intentional.
- Once semantic typing and runtime representation are properly separated, Flux can evaluate whether Aether and CFG should carry richer typed metadata for optimization without threatening the single-semantic-IR invariant.
