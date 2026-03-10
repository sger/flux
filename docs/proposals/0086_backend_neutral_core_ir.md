- Feature Name: Backend-Neutral Core IR
- Start Date: 2026-03-08
- Status: Draft
- Proposal PR:
- Flux Issue:

# Proposal 0086: Backend-Neutral Core IR

## Summary
[summary]: #summary

Introduce a backend-neutral **Core IR** as the canonical semantic lowering layer between
Flux's typed AST and the execution backends (VM bytecode and Cranelift JIT).

The Core IR becomes the single internal representation for:

1. explicit control flow
2. function calls and closures
3. effect operations and handler boundaries
4. actor operations
5. typed/runtime boundary checks
6. Aether ownership and reuse annotations

The VM and JIT must both lower from this IR rather than encoding Flux semantics through
separate ad hoc lowering paths.

This proposal does **not** replace bytecode, Cranelift IR, or the typed AST. It inserts a
shared semantic layer between them.

## Motivation
[motivation]: #motivation

Flux is now large enough that direct lowering from AST-ish structures into backend-specific
execution formats is becoming a structural liability.

Current and near-future pressure points:

- typed AST and explicit typing artifacts ([0046](implemented/0046_typed_ast_hm_architecture.md))
- compiler phase modularization ([0044](0044_compiler_phase_pipeline_refactor.md))
- actor concurrency ([0065](0065_actor_effect_stdlib.md),
  [0066](0066_thread_per_actor_handler.md))
- evidence-passing handlers ([0072](0072_evidence_passing_handlers.md))
- Aether ownership/reuse ([0084](0084_aether_memory_model.md))
- type-informed optimization ([0077](0077_type_informed_optimization.md))

Without a shared Core IR, Flux risks:

1. **VM/JIT semantic drift**: effects, actors, and boundary checks become duplicated logic.
2. **Optimization fragmentation**: ownership/reuse and effect-aware optimizations must be
   implemented separately per backend.
3. **Debugging blind spots**: no single representation exists where developers can inspect
   "what the compiler believes the program means" before backend lowering.
4. **Compiler complexity growth**: more semantics accumulate in bytecode emission paths and
   JIT-specific lowering code.

Flux needs one internal layer where language semantics are explicit before backend details
take over.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### What the Core IR is

The Core IR is a compiler-internal representation that sits between:

- typed/high-level language structure
- low-level execution formats

Pipeline shape:

```text
source
  -> parse
  -> rename/desugar
  -> HM/effect typing
  -> TypedProgram
  -> Core IR
  -> VM bytecode
  -> Cranelift IR / machine code
```

The key idea is:

> Flux semantics should be decided once, in one representation, before backend-specific
> lowering begins.

### What moves into the Core IR

The Core IR should make the following explicit:

- branch and merge points
- locals/temporaries
- function calls
- closure creation
- ADT/tuple/array/list/map construction
- pattern-match lowering results
- effect perform operations
- handler boundaries and handler arms
- actor operations such as spawn/send/recv
- typed/runtime boundary checks
- ownership/reuse hints relevant to Aether

### What does not belong in the Core IR

The Core IR is not:

- syntax-shaped AST with minor tweaks
- raw VM bytecode
- Cranelift IR
- machine-specific
- register-allocator-specific
- final memory layout encoding

### Why this helps Flux specifically

#### Effects and handlers

Handlers are a semantic feature, not a VM trick or a JIT trick. Their lowering should be
decided once.

#### Actors

Actor operations should exist in one representation before they become bytecode ops or JIT
runtime helper calls.

#### Aether

Ownership evidence and reuse candidates should attach to one semantic representation, not
be rediscovered independently in multiple backends.

#### VM/JIT parity

If VM and JIT both lower from the same Core IR, semantic parity becomes easier to test and
maintain.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Position in the compiler pipeline

Recommended pipeline:

1. Parse to AST
2. Apply source-to-source desugaring and validation-preserving rewrites
3. Produce `TypedProgram` and effect artifacts
4. Lower `TypedProgram` to Core IR
5. Run Core IR validation / optional optimization passes
6. Lower Core IR to:
   - VM bytecode
   - Cranelift IR through JIT lowering

The VM remains the authoritative semantics. The Core IR does not change that; it makes the
shared semantics explicit before backend emission.

### Suggested representation style

The Core IR should be:

- function-oriented
- block-based
- control-flow explicit
- value-oriented
- rich enough to preserve source spans and selected type/effect metadata

Recommended shape:

```text
CoreProgram
  functions: Vec<CoreFunction>

CoreFunction
  name
  params
  blocks
  result_type
  effect_row

CoreBlock
  id
  instructions: Vec<CoreInstr>
  terminator: CoreTerminator
```

This is intentionally closer to MIR-style CFG than to stack-machine bytecode.

### Core IR instruction categories

#### 1. Value construction

- constants
- tuple construction
- array construction
- ADT construction
- closure construction

#### 2. Data access

- local get/set
- tuple projection
- ADT field extraction
- array/list/map internal access nodes

#### 3. Calls

- direct call
- indirect call
- Base/PrimOp call

Public API names are already resolved before this layer. The Core IR should not need to
model unresolved names.

#### 4. Control flow

- branch on boolean
- jump
- return
- match already lowered into tests plus branches

#### 5. Effects and handlers

- `Perform { effect, op, args }`
- `EnterHandler { effect, handler_id }`
- `Resume { continuation, value }`
- handler exits / discharge points

The exact encoding can vary, but effectful control flow must be explicit here.

#### 6. Actor operations

- `ActorSpawn`
- `ActorSend`
- `ActorRecv`

These may later lower to `Perform Actor.spawn/send/recv` or dedicated runtime calls, but
the Core IR must represent them explicitly enough for both backends.

#### 7. Boundary checks

- module contract checks
- strict typed boundary checks
- effect sealing / capability enforcement checks

This ensures runtime-visible failures come from one lowering model.

#### 8. Aether annotations

The Core IR should be able to attach ownership/reuse facts, for example:

```text
Ownership = Fresh | Unique | Shared | Escaped | Unknown
```

and optional reuse metadata:

```text
ReuseCandidate { kind, source_value, fallback_policy }
```

These do not need to be first-class instructions in all cases; metadata on values or nodes
is acceptable. What matters is that this information exists before backend lowering.

### Relationship to typed AST

The typed AST remains valuable for:

- syntax-oriented diagnostics
- type/effect attachment
- source-structured analysis

The Core IR should not replace that role. It should consume typed information and lower it
into execution-oriented form.

### Relationship to bytecode

Bytecode remains:

- the VM execution format
- compact and cacheable
- stack-machine-oriented

The Core IR is not a replacement for bytecode tooling such as
[0023](0023_bytecode_decode_passes.md).
Instead, it changes where semantic lowering happens:

```text
Today: typed/compiler logic -> bytecode directly
Target: typed/compiler logic -> Core IR -> bytecode
```

### Relationship to Cranelift IR

Cranelift IR remains the JIT backend representation. It should not be the place where Flux
semantics are first decided.

Target:

```text
Core IR -> Cranelift lowering -> runtime helper calls / machine code
```

not:

```text
AST-ish lowering -> ad hoc Cranelift generation
```

### Required metadata

At minimum, Core IR nodes or values should preserve:

- source span
- inferred result type where useful
- function effect row
- purity/effect summary where needed for lowering
- ownership/reuse metadata where available

This is important for diagnostics, debug tooling, and parity investigation.

### Validation requirements

Add a Core IR validator that checks invariants before backend lowering:

1. all referenced locals/blocks exist
2. terminators are well-formed
3. type/effect metadata required by backend passes is present
4. ownership annotations are internally consistent where emitted
5. effect/handler structures are structurally valid

This validator should be runnable independently in tests and debug tooling.

### Debug/tooling interaction

The Core IR should become inspectable through future tooling, especially
[0076](0076_debug_toolkit.md).

Recommended future commands:

- `flux analyze --core-ir file.flx`
- `flux analyze --core-ir --jit file.flx`

This gives developers a semantic debugging view before bytecode or Cranelift details.

### Optimization policy

The Core IR is the preferred home for:

- type-informed optimizations
- effect-aware simplifications
- ownership/reuse preparation
- control-flow simplification

It is not necessarily the home for:

- bytecode peephole rewrites
- machine-level scheduling
- register allocation concerns

Those remain backend-specific.

### Rollout plan

#### Phase 1: behavior-preserving introduction

- add Core IR data structures
- lower a subset of existing functions to Core IR
- immediately lower Core IR to current bytecode without semantic changes
- keep JIT path on existing lowering temporarily if needed

#### Phase 2: bytecode backend cutover

- make VM bytecode generation consume Core IR universally
- preserve diagnostics and runtime behavior

#### Phase 3: JIT backend cutover

- lower JIT from Core IR as the canonical source
- remove duplicated semantic lowering logic where possible

#### Phase 4: Core IR optimization and tooling

- add validator
- add analysis dumps
- attach Aether ownership evidence
- add effect/actor lowering tests at Core IR level

## Drawbacks
[drawbacks]: #drawbacks

1. Introduces another internal compiler representation to maintain.
2. Requires careful migration to avoid semantic drift during transition.
3. May temporarily slow compiler development while old and new lowering paths coexist.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why not keep lowering directly to bytecode and Cranelift separately?

Because Flux is now too semantically rich for duplicated lowering to remain safe. Effects,
handlers, actors, and Aether all increase the cost of duplicated semantics.

### Why not use bytecode itself as the shared IR?

Because bytecode is a stack-machine execution format, not an ideal semantic and
optimization representation. It is too low-level for clear ownership/effect/handler
reasoning and too VM-shaped to serve as the best common backend contract.

### Why not use Cranelift IR as the shared representation?

Because Cranelift IR is a backend IR, not a Flux semantic IR. The VM would then become a
second-class target, and Flux semantics would be overfit to one backend's structure.

## Prior art
[prior-art]: #prior-art

- Rust's HIR/MIR/backend split
- compilers that lower rich source semantics into a backend-neutral SSA/CFG form
- functional language compilers that separate effect/control lowering from machine IR

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Should the Core IR be SSA-based from day one, or use mutable locals with CFG blocks
   first for lower implementation risk?
2. Which effect-handler constructs should be explicit instructions versus metadata-driven
   lowering forms?
3. Should actor ops be represented as generic effect performs or dedicated Core IR
   instructions?
4. How much type metadata should remain on every Core IR value versus only at selected
   boundaries?
5. Should Core IR be serializable for offline tooling or cache experiments?

## Future possibilities
[future-possibilities]: #future-possibilities

- Core IR snapshots in the test suite
- serialization for cache/debug pipelines
- Aether-specific optimization passes over Core IR
- backend-independent optimization reports
