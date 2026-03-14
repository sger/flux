- Feature Name: Base + Flow — Auto-Imported Prelude and Standard Library
- Start Date: 2026-02-12
- Status: Partially Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0029: Base + Flow — Auto-Imported Prelude and Standard Library

## Summary
[summary]: #summary

Replace the hard-coded base system with a **`Base` module** — a privileged module that is auto-imported into every Flux module and script. Functions in `Base` look and feel like language primitives but participate in the module system. Users can exclude specific `Base` functions. The standard library grows as **`Flow`** modules — data flows through Flux.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Phase 2: Import Except Syntax (Medium Risk)

| Step | What | Effort |
|------|------|--------|
| 2.1 | Add `except` field to `Statement::Import` | Small |
| 2.2 | Parse `import Base except [...]`; reject `import Base as X` | Small |
| 2.3 | Update walk_stmt/fold_stmt for new field | Small |
| 2.4 | Compiler: skip excluded names during Base injection | Small |
| 2.5 | Enable qualified `Base.name(...)` via synthetic module resolution | Medium |
| 2.6 | Add `--no-base` CLI flag to disable prelude injection entirely | Small |
| 2.7 | Tests for except, qualified access, and `--no-base` | Small |

**Milestone:** Users can exclude and qualify Base functions. `--no-base` enables minimal/sandbox environments.

The `--no-base` flag is useful for:
- Sandboxed execution (no `print`, no I/O)
- Teaching ("look, everything is just a module — even `len`")
- Embedded/minimal runtimes
- Testing Base itself

### Phase 2: Import Except Syntax (Medium Risk)

### Phase 2: Import Except Syntax (Medium Risk)

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **__preamble__:** **Superseded by:** `0028_base.md`, `0030_flow.md` - **Problem 1: Base Functions Bypass the Module System:** Current base functions are a parallel universe. T...
- **Detailed specification (migrated legacy content):** > Historical combined context document. > Canonical current split: > - Base: `docs/proposals/implemented/0028_base.md` > - Flow: `docs/proposals/0030_flow.md`
- **Problem 1: Base Functions Bypass the Module System:** Current base functions are a parallel universe. They're registered by index in the compiler, dispatched via `OpGetBase`, and completely invisible to the module system: ```rust /...
- **Problem 2: No User Control Over Global Names:** Every base is unconditionally global. Users cannot: - Shadow a base with a local definition without a shadowing warning - Exclude a base they don't want (e.g., a `print` that co...
- **Problem 3: No Clear Boundary Between "Essential" and "Library":** All 42 base functions have equal status. But `print` and `len` are fundamentally different from `starts_with` and `slice`. There's no layering that says "these are the core voca...
- **Problem 4: Growing Pains:** Proposals 017 (GC/collections) and 026 (concurrency) will add 10+ new base functions each (`hd`, `tl`, `list`, `put`, `get`, `spawn`, `send`, `ask`, `await`, etc.). Without a pr...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### Phase 2: Import Except Syntax (Medium Risk)

| Step | What | Effort |
|------|------|--------|
| 2.1 | Add `except` field to `Statement::Import` | Small |
| 2.2 | Parse `import Base except [...]`; reject `import Base as X` | Small |
| 2.3 | Update walk_stmt/fold_stmt for new field | Small |
| 2.4 | Compiler: skip excluded names during Base injection | Small |
| 2.5 | Enable qualified `Base.name(...)` via synthetic module resolution | Medium |
| 2.6 | Add `--no-base` CLI flag to disable prelude injection entirely | Small |
| 2.7 | Tests for except, qualified access, and `--no-base` | Small |

**Milestone:** Users can exclude and qualify Base functions. `--no-base` enables minimal/sandbox environments.

The `--no-base` flag is useful for:
- Sandboxed execution (no `print`, no I/O)
- Teaching ("look, everything is just a module — even `len`")
- Embedded/minimal runtimes
- Testing Base itself

### Phase 1: BaseModule Struct (Low Risk)

| Step | What | Effort |
|------|------|--------|
| 1.1 | Create `src/runtime/core.rs` with `BaseModule` struct | Small |
| 1.2 | Populate with all 42 current base functions by name | Small |
| 1.3 | Replace `define_base` loop in compiler with `BaseModule` iteration | Small |
| 1.4 | Remove hard-coded index constants | Small |
| 1.5 | All tests pass, zero behavioral change | — |

**Milestone:** Single source of truth for base functions. No more index coupling.

### Phase 2: Import Except Syntax (Medium Risk)

### Phase 3: Flow Library Infrastructure (Medium Risk)

| Step | What | Effort |
|------|------|--------|
| 3.1 | Add virtual module registry to module resolver | Small |
| 3.2 | Embed Flow sources via `include_str!` in compiler binary | Small |
| 3.3 | Write `Flow.List` module | Medium |
| 3.4 | Write `Flow.Option` module | Small |
| 3.5 | Write `Flow.Either` module | Small |
| 3.6 | Write `Flow.Func` module | Small |
| 3.7 | Write `Flow.Math` module (after operators land) | Small |
| 3.8 | Write `Flow.String` module | Small |
| 3.9 | Write `Flow.Dict` module | Small |
| 3.10 | Precompile Flow modules to `.fxc` cache | Small (reuse existing cache) |
| 3.11 | Integration tests for all Flow modules | Medium |

**Milestone:** `import Flow.List` works out of the box.

### Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| Non-deterministic base ordering | Bytecode cache invalidation, wrong function called | Use `Vec` for ordered registry; never derive indices from `HashMap` iteration |
| Breaking bytecode cache | Stale cache loads wrong base functions | Bump cache version in Phase 1 |
| `except` conflicts with future keyword | Syntax ambiguity | `except` is contextual — only meaningful after `import Base` |
| Flow module performance | Pure Flux slower than native | Profile; promote hot paths to Base if needed |
| Name collisions between Base and Flow | `map` in Base vs `Flow.Option.map` | Base is unqualified; Flow is always qualified by module name |
| Circular imports in Flow | `Flow.List` uses `Flow.Option`? | Keep Flow modules independent; each only uses Base |

### Phase 1: BaseModule Struct (Low Risk)

### Phase 2: Import Except Syntax (Medium Risk)

### Phase 3: Flow Library Infrastructure (Medium Risk)

**Milestone:** `import Flow.List` works out of the box.

### Risks

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Decision Framework

A function belongs in **Base** if:
1. It requires VM access (print, map, filter, fold — need `RuntimeContext` for callbacks)
2. It operates on primitive types with no pure-Flux equivalent (type_of, is_int, to_string)
3. It needs native performance for correctness (sort, len on strings — Unicode)
4. It's used so universally that requiring an import would be tedious (len, first, rest)

A function belongs in **Flow** if:
1. It can be implemented in Flux using Base functions
2. It's a combinator or convenience wrapper (take, drop, zip, compose, identity)
3. It's domain-specific (math functions, string utilities)
4. It's used in specific contexts, not universally

### Decision Framework

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### References

- [Elixir Kernel](https://hexdocs.pm/elixir/Kernel.html) — auto-imported module, `import Kernel, except: [...]`
- [Haskell Prelude](https://hackage.haskell.org/package/base/docs/Prelude.html) — implicit import, `import Prelude hiding (...)`
- [Gleam prelude](https://hexdocs.pm/gleam_stdlib/) — pure FP on BEAM, auto-imported core
- [Elm Core](https://package.elm-lang.org/packages/elm/core/latest/) — `Basics` module auto-imported
- [Lua standard libraries](https://www.lua.org/manual/5.4/manual.html#6) — global functions + explicit `require`

### References

## Unresolved questions
[unresolved-questions]: #unresolved-questions

### Open Questions

1. **Should `map`/`filter`/`fold` be polymorphic over List (proposal 017)?** Currently they only work on Array. If they become polymorphic, they stay in Base. If List gets separate `List.map`/`List.filter`, those go in Flow.List. Recommendation: polymorphic in Base — match Elixir's `Enum` approach.

2. **Should `except` work on non-Base imports?** E.g., `import Flow.List except [take]`. This is useful but adds parser complexity. Recommendation: defer — only Base gets `except` initially.

3. ~~**Flow embedding strategy?**~~ Resolved: embed via `include_str!`, virtualize in the module resolver. No filesystem dependency. Users can override by placing their own `.flx` on disk (local file wins).

4. **Should `Flow` be `Flux`?** E.g., `import Flux.List` instead of `import Flow.List`. More branded but risks confusion with the language name. Recommendation: `Flow` — distinct from the language, but clearly related.

5. **Should Base be `Flux.Base`?** In Elixir it's just `Kernel`, not `Elixir.Kernel`. Recommendation: just `Base` — simpler.

6. ~~**How does `--no-base` flag work for minimal environments?**~~ Resolved: `--no-base` is a Phase 2 deliverable (step 2.6). Disables prelude injection entirely.

### Open Questions

## Future possibilities
[future-possibilities]: #future-possibilities

### Future Base Additions

As new proposals land, Base grows:

```
// Proposal 0017: Persistent Collections
Base.hd(list)
Base.tl(list)
Base.list(...)
Base.is_list(x)
Base.is_map(x)
Base.put(map, k, v)
Base.get(map, k)

// Proposal 0026: Concurrency (Layer 1 only)
Base.spawn(fn)
Base.send(actor, msg)
Base.receive()
```

Each addition is a single entry in the Base module definition — no index synchronization.

### Future Base Additions

As new proposals land, Base grows:
