# Changelog

All notable changes to Flux are documented here. See [docs/versions/](docs/versions/) for detailed What's New guides per release.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

---

## [Unreleased]

### Added
- `scripts/changelog/check_changelog_fragment.sh` to enforce changelog fragments in PR CI.
- `scripts/changelog/changelog_from_fragments.sh` to rebuild `CHANGELOG.md` `[Unreleased]` from `changes/*.md`.
- `scripts/release/release_cut.sh` to cut a new version section from `[Unreleased]` and update compare links.
- `scripts/release/release_check.sh` local preflight command documented in `README.md`.
- add proposals for grammar improvements, deterministic effect replay, typed module contracts, and macro system
- Effect-row constraint solver (`src/bytecode/compiler/effect_rows.rs`): `EffectRow`, `RowConstraint`, `RowSolution`, and `solve_row_constraints` implementing set-based row arithmetic with var binding, link propagation, and worklist-based resolution.
- New error codes `E419` (unresolved single effect variable), `E420` (ambiguous multiple effect variables), `E421` (invalid effect subtraction), `E422` (unsatisfied effect subset) with deterministic sorted diagnostics.
- Pass fixtures: `100_effect_row_order_equivalence_ok.flx`, `101_effect_row_subtract_concrete_ok.flx`, `102_effect_row_subtract_var_satisfied_ok.flx`, `103_effect_row_multivar_disambiguated_ok.flx`, `104_effect_row_absent_ordering_linked_ok.flx`.
- Fail fixtures: `194_effect_row_multi_missing_deterministic_e400.flx`, `195_effect_row_invalid_subtract_e421.flx`, `196_effect_row_subtract_unresolved_single_e419.flx`, `197_effect_row_subtract_unresolved_multi_e420.flx`, `198_effect_row_subset_unsatisfied_e422.flx`, `199_effect_row_subset_ordered_missing_e422.flx`, `200_effect_row_absent_ordering_linked_violation_e421.flx`.
- Tiered Flux example execution in CI via manifest-driven runs (`ci/examples_manifest.tsv`) and runner automation (`scripts/ci/run_flux_manifest.sh`).
- New contextual boundary/effect fixtures for type-system hardening (`161`, `189`, `190`, `191`).
- New parser contextual recovery fixtures and snapshots for perform/handle/module structural diagnostics.
- add N-ary Core IR pipeline, fix CFG bytecode compilation, and update docs
- switch JIT calls to tagged values
- move non-nullary ADTs onto GC heap
- add cross-language benchmark suite and runtime updates
- add type-informed optimizations and stable HM expr ids
- add type-informed folding and ExprId-based HM lookups
- add type-informed AST optimization pass and stable ExprId typing
- add type-informed AST folding and expr-id based HM typing
- add stable IDs to parsed expression nodes
- switch compose to lazy normalization
- add inference for perform/handle and lambda expressions
- 0051 Stage 2 — HM fallback for generic/ADT contract params
- tighten HM signatures for collection, map, list, and misc builtins
- add effect-row-aware HM unification and substitutions
- enforce explicit effect row tails and document parser behavior
- parse explicit effect row tails and document parse_effect_expr
- support row variables in effect expressions and document effect-row completeness
- add row-constraint solver coverage and deterministic diagnostics
- FluxRep on Core IR, typed LLVM codegen, and worker/wrapper split (Proposal 0119 Phases 1-3)
- delete src/runtime/base/, primops as single source of truth (Proposal 0120 Phase 4)
- Flux standard library and primop promotion (Proposal 0120 Phases 1-3)
- auto-import Base.Option prelude for all Flux programs (Proposal 0120 POC)
- module interface files and per-module type inference for native backend (Proposal 0121 Phases 2-3)
- `exposing` import syntax, native stack traces, and division-by-zero fix (Proposal 0121 Phase 1)
- CPS continuation stack for non-tail recursion (Proposal 0122 Phase 3)
- trampoline-based mutual tail call optimization (Proposal 0122 Phase 2)
- guaranteed self-tail-call optimization (Proposal 0122 Phase 1)
- AoC 2024 Days 1-5 native, SSA fix, Proposal 0122
- add MemberAccess resolution for module-qualified calls
- higher-order closure calls, 50+ C runtime builtins (Phase 9d)
- add 30+ C runtime builtins and fix remaining parity (Phase 9d)
- add to core to llvm primop expansion and flux base
- add pipeline, CLI integration, builtins, and proposals (Phase 8+)
- add Aether RC integration (Phase 7)
- add ADT lowering and pattern matching
- add closure lowering and higher-order calls
- lower top-level core functions and control flow
- add prelude and arithmetic helper emission
- add LLVM text IR AST and pretty-printer
- CFG covers all well-typed functions — Handle, MakeHash, pre-validation
- CFG primary compilation path — fix MakeList, enable Prefix/LoadName
- FBIP annotations, CFG binary op coverage, typed-let pre-validation
- spine-based dup/drop fusion, rt_index list/string support, deterministic bench output
- CFG bytecode expansion with pre-codegen validation and JIT empty list
- --dump-aether flag, CFG bytecode handlers, pre-codegen validation
- Aether non-linear control flow safety
- Aether cross-function borrowed parameters
- Aether drop-reuse specialization
- Aether drop specialization — IsUnique across all three backends
- Aether alignment  reuse specialization, VM opcodes, drop specialization
- Aether scrutinee-drop enable reuse in practice
- Aether reuse — operational runtime integration
- make Aether operational across all three backends
- Aether Phase 7 — reuse token analysis in Core IR
- Aether Phase 6 — borrowing analysis for dup/drop elision
- Aether Phase 5 — compile-time dup/drop insertion
- Aether Phase 5 — remove GcHeap and complete memory model migration
- Aether Phase 4 — remove Value::Gc and legacy GC allocation paths
- Aether Phase 3 — migrate HAMT maps from GcHeap to Rc
- Aether Phase 2 — eliminate Value::GcAdt, unify all ADTs under Rc
- Aether Phase 1 — migrate cons lists from GcHeap to Rc<ConsCell>
- implement Aether Phase 0 — Rc reuse fast paths for base functions and VM opcodes

### Changed
- CI now runs changelog fragment validation on pull requests.
- Release docs now use a fragment-first changelog workflow.
- Refactor base function handling and introduce fastcall allowlist
- fix clippy issues
- update README.md
- add changelog fragment
- update snapshot tests
- Refactor built-in functions to base functions and update related tests
- Refactor terminology from "builtins" to "base functions" across documentation and examples
- Rename builtins to base functions and update related references for consistency
- Supports module imports with member exclusions
- Refactor built-in functions to use a consistent naming convention
- update documentation
- Migrates runtime terminology to Base-first and documents API
- Implement string manipulation builtins and type checking functions
- rename builtins to base functions and update related imports and calls
- add new error codes for Base directive handling and update existing error message
- update bytecode compiler for Base module integration and error handling
- enhance import statement handling in AST folding and visiting
- add fragment about changelog
- fix changelog script
- fix unit test
- update snapshots
- Add documentation comparing PrimOps and Builtins with routing rules
- Add comprehensive PrimOps tests and examples
- Add extended primitive operations and their execution logic
- Add tests for PrimOp functionality in compiler and JIT
- Add error handling for juxtaposed identifiers in parser
- Add null value handling in compile_statement to return early
- Refactor leave_scope to return EffectSummary and update debug info handling in compiler
- Enhance effect summary handling in compiler and add tests for primitive operations
- Add proposals for deterministic effect replay, typed module contracts, and hygienic macro system
- Add examples for primitive operations and effect boundaries in the Flux runtime
- Add rt_call_primop to runtime symbols for primitive operation support
- Add support for primitive operations in the compiler and JIT
- Add rt_call_primop function for executing primitive operations with error handling
- Enhance documentation for execute_primop_opcode function with detailed error handling descriptions
- Add comprehensive primitive operation support with additional arithmetic, comparison, and utility functions
- Implement primitive operation support with OpPrimOp and OpCallBuiltin opcodes
- Add PrimOp and PrimEffect enums to support primitive operations
- `collect_effect_row_constraints` and `collect_effect_expr_absence_constraints` in `expression.rs` integrate the new solver for all call-site effect-row validation (subset checks, absence constraints, unresolved-var detection).
- CI manifest (`ci/examples_manifest.tsv`) extended with all new pass/fail fixtures (tier 2, both VM and JIT).
- Extended example fixture manifest and snapshot coverage to keep these diagnostics/warnings stable in CI.
- Locked contextual diagnostics output for 0058 call-site/let/return mismatches with explicit snapshot coverage.
- Strengthened parser contextual message/recovery regression guards for targeted `E034` paths (perform/handle/handle-arm/module).
- fix clippy errors
- Introduce Flux IR lowering and route JIT through it
- drive truthiness branching from HM expression types
- reorganize binarytrees benchmarks and add smoke/full profiling workflow
- convert Error to error lowecase
- Harden JIT runtime diagnostics and add row-effect error fixtures
- Improve JIT runtime diagnostic parity for base and primop errors
- Improve parser diagnostics and add architecture proposals
- Normalize type system example numbering
- Harden parser diagnostics emission and refresh snapshots
- Polish grouped diagnostics headers and note rendering
- Complete diagnostics quality proposal 0080
- update tests
- harden parser metadata and runtime stack traces
- add rustdocs for diagnostics module
- update and add new proposals
- update unit and snapshot tests
- Improve effect diagnostics and source rendering
- Unify compiler, VM, and JIT diagnostic behavior
- Tighten runtime base helper behavior and tests
- Improve parser recovery and contextual diagnostics
- Refactor diagnostics around shared quality and taxonomy helpers
- Add diagnostic category type to diagnostics model
- Refine diagnostic builder support for structured rendering
- Refine diagnostics JSON and source snippet rendering
- update diagnostics renderer
- Fix effect row propagation in inference and bytecode compilation
- add new proposal
- review and update proposals
- rename and clean up src/types/ module
- split unify_error.rs into error types and algorithm modules
- introduce constraint solver and improve diagnostics
- Add stable expression IDs to parsed AST nodes
- unify fold helpers and simplify free-var scope tracking
- add quality/perf guards and docs proposals
- remove ignore unit tests
- remove legacy expression module and add infer config struct
- implement modular expression inference pipeline
- add collection and control-flow expression inference modules
- split calls inference helpers into new file
- split calls inference helpers and unify naming
- modularize statement/pattern inference and self-call search
- split pattern/statement inference helpers and unify naming
- type-infer: split expression inference module and wire new API
- type-infer: document ADT/effect helpers and split call-effect constraint paths
- update docs
- close proposals
- Split monolithic 2,250-line type_infer.rs into a module directory with one file per concern
- update function name
- update unit tests for type inference
- update changelog fragment
- update examples and docs
- fix unit tests
- improve effect-row inference for function arguments and HM integration
- improve runtime error boundary for vm and jit
- rename nary→core, replace ir/ with backend_ir facade, restructure IR pipeline
- fix examples for vm and jit output
- Fix JIT runtime error rendering for arithmetic ops to match VM diagnostics
- add test framework assertions and comprehensive syntax test suite
- add changelog from commits
- add TBAA metadata to LLVM backend loads/stores
- add inlinehint to small frequently-called runtime helpers
- enable LLVM tailcallelim for self-recursive tail calls
- add LLVM function attributes to rt_* helper declarations
- extract shared backend metadata collection to backend_ir/metadata.rs
- add proper purity analysis for Core IR optimizer
- extend runtime error checks to comparisons and prefix ops in JIT/LLVM
- fix type-directed arithmetic: require proven operand types for IAdd/FAdd
- add  tail call optimization for llvm
- update snapshots tests
- refactor LLVM compiler into focused submodules
- LLVM backend: fix parity issues and add runtime error reporting
- fix runtime contract
- remove synthetic stack traces from jit and llvm
- add runtime extraction the LLVM backend is now fully independent from the cranelift JIT
- fix parity issues between llvm and jit
- implement LLVM backend phase 6: AOT object file emission
- implement LLVM backend phase 4: effect handlers and Perform expression
- implement LLVM backend phase 3: ADTs, pattern matching, and collections
- complete LLVM backend phase 2: prefix ops, interpolation, and all binary ops
- fix string constants, closures with captures and engine cleanup crash
- add HandleScope compilation (ops array, closures, push_handler call)
- add compilation support for print
- add scaffold llvm module and install dependencies
- migrate backend_ir to cfg module
- move VM executor from runtime/ to bytecode/ for backend symmetry
- extract compile() into explicit phase pipeline (proposal 0044 M1)
- Add Flow.Array module (Proposal 0125 Phase 2) and fix LLVM module name collisions
- fix advanced examples
- add new proposal and update parity script to show cargo run commands
- Add builtin mappings to flux_rt_eq/flux_rt_neq which already existed in the C runtime with deep structural comparison.
- fix snapshot tests
- format code
- move base files to flow stdlib
- update proposals
- Improve LLVM lowering for unboxed values and int coercions
- cleanup src/runtime/base/
- fix linking issues on linux
- auto build libflux
- update gitignore
- remove Cranelift JIT and llvm-sys backends (Proposal 0118)
- add c runtime
- fix issues phase 5
- fix issues phase 4 and 5
- improve phase 1 and 2
- Add iterative Core simplifier gated behind -O flag (Proposal 0112 Phase 1)
- Fix nested if-else bytecode jump target overrun and add VM IP guard
- Split JIT compiler into submodules and fix CI regressions
- Refine Aether reuse/fusion analysis and document proof scaffold
- fix snapshots
- Bounded FBIP forms, deferred until after N-U
- close the remaining reuse gap on transparent-wrapper and forwarding-child style patterns
- add cli command for aether report
- fix vm bug regarding spans
- make Aether maturity measurable on a broader workload
- strengthen interprocedural ownership and FBIP summaries so more direct/internal/imported cases compose precisely.
- increase `DropSpecialized` coverage on structurally safe cases that still stay conservative today.
- recover more profitable `Reuse` sites that are still being missed in realistic transformed Core.
- reduce conservative outcomes that still appear in recursive and higher-order functions.
- fix parser issue with fbip
- broaden the set of realistic transformed Core shapes that trigger profitable reuse and drop specialization.
- improve borrow/liveness precision toward Koka-style behavior across recursive, higher-order, and imported/base-call cases.
- fix unit and snapshot tests
- complete pipeline and strict effect enforcement
- switch dup/drop insertion to environment-based planning
- update proposal
- remove repl
- Implement proposal 0137 modular interface and VM cache pipeline

### Fixed
- Hardened strict type/effect diagnostics for unresolved `perform` argument paths (locked with new failing fixture `192_perform_arg_unresolved_strict_e425.flx`).
- Added regression coverage for unreachable pattern-arm warnings via new fixture `193_unreachable_pattern_arm_w202.flx`.
- JIT release parity regression for AOC day05 part1 test caused by invalid do-block control-flow emission in JIT lowering.
- resolve 6 compiler bugs and restore lost diagnostics
- align closure ABI and tagged collection helpers
- preserve arena-backed values across GC
- clarify type variable allocator naming
- rename type var alloc helpers for clarity
- prevent HM substitution cycles from hanging call-base tests
- wire Base HM signatures into registry entries
- clarify row variable allocator naming
- rename row-var fresh counter for clarity
- accept untyped closures for function-typed parameters
- local variables shadow base functions in call resolution
- unify call ABI, fix 5 parity gaps, add effect handler tests
- close 16 JIT coverage gaps and add automated VM/JIT parity test
- BigInt equality, parse_ints builtin, and --dump-core for multimodule programs
- write all fields in Aether reuse to fix list [0,0,0] bug
- bigint overflow, MemberAccess, malloc GC, AoC Day 6 (Phase 9e)
- HAMT, comparisons, indexing, to_string — zero mismatches (Phase 9c)
- runtime arithmetic dispatch, string concat, tuple tags (Phase 9b)
- fix closure SSA, string names, ADT printing, cache bypass (Phase 9a)

### Docs
- Added `changes/README.md` and `changes/_template.md` for contributor guidance.
- Updated type-system/effects documentation across guides and internals for v0.0.4 alignment.
- Refreshed proposal set for post-0.0.4 planning lanes, including effect-row variables, actor/effect tracks, and uniqueness/performance follow-ups.
- Improved cross-references between roadmap/proposals and implementation evidence sections.
- Proposals `0042` and `0049` marked `Implemented | have` in `docs/proposals/0000_index.md` with full closure evidence.
- `examples/type_system/README.md` and `examples/type_system/failing/README.md` updated with new fixture entries and 0049 run-command section.
- Updated v0.0.4 roadmap evidence and task status tracking (`R4-T01` through `R4-T09`).
- Expanded proposal tracking/docs for deferred and post-0.0.4 lanes.
- move implemented proposals into dedicated folder

---

## [v0.0.3] - 2026-02-21

### Added
- `scripts/changelog/check_changelog_fragment.sh` to enforce changelog fragments in PR CI.
- `scripts/changelog/changelog_from_fragments.sh` to rebuild `CHANGELOG.md` `[Unreleased]` from `changes/*.md`.
- `scripts/release/release_cut.sh` to cut a new version section from `[Unreleased]` and update compare links.
- `scripts/release/release_check.sh` local preflight command documented in `README.md`.
- **Cranelift JIT backend** — compiles Flux programs to native machine code (`--features jit`, `--jit` flag)
- **Persistent cons lists** — GC-managed immutable linked lists with O(1) prepend; `[h | t]` literal and pattern syntax
- **HAMT hash maps** — Hash Array Mapped Trie with structural sharing on update; original maps are never mutated
- **Tuples** — `(a, b)` literals, `.0`/`.1` field access, destructuring in `let` and `match`
- **Do-blocks** — `do { expr1; expr2 }` sequential expression blocks; last expression is the value
- **Where clauses** — `expr where x = val` local bindings scoped after the body expression; multiple clauses chain
- **List comprehensions** — `[x * 2 | x <- xs, x > 0]` desugared at parse time to `map`/`filter`/`flat_map`
- **Pattern guards** — `match n { x if x > 0 -> ..., _ -> ... }`
- **Cons patterns** — `match lst { [h | t] -> ..., _ -> ... }`
- **Unit test framework** — `test_*` function discovery, `--test` / `--test-filter` flags, exit code 0/1
- **`Flow.FTest` stdlib** — `describe`, `it`, `for_each`, `with_fixture`, `approx_eq` helpers (`lib/Flow/FTest.flx`)
- **Mark-and-sweep GC** — manages cons cells and HAMT nodes; configurable threshold (`--gc-threshold`)
- **`--stats` flag** — timing and code metrics: parse, compile, execute, modules, source lines, instructions
- **`--optimize` / `-O`** — AST optimization passes: desugaring → constant folding → alpha-renaming
- **`--analyze` / `-A`** — analysis passes: free variable collection and tail-call detection
- **`--no-gc`** — disable garbage collection
- **`--gc-telemetry`** — GC stats on exit (requires `--features gc-telemetry`)
- **`--leak-detector`** — print allocation stats on exit
- **New CLI subcommands**: `analyze-free-vars`, `analyze-tail-calls`, `cache-info-file`
- **New builtins (75 total, up from 35)**: `flat_map`, `zip`, `find`, `any`, `all`, `count`, `sort_by`, `first`, `last`, `rest`, `range`, `sum`, `product`, `hd`, `tl`, `list`, `is_list`, `to_list`, `to_array`, `read_file`, `read_lines`, `read_stdin`, `parse_int`, `parse_ints`, `split_ints`, `now_ms`, `time`, `assert_eq`, `assert_neq`, `assert_true`, `assert_false`, `assert_throws`, `starts_with`, `ends_with`, `replace`, `flatten`, `is_map`
- **`dev-fast` build profile** — opt-level 3 with lighter debug info
- **VS Code syntax highlighting** extension (`tools/vscode/`)
- **62 opcodes** (up from 44)

### Changed
- Array syntax: `[| 1, 2, 3 |]` for arrays (Rc-backed); `[1, 2, 3]` now creates cons lists
- Arrays and cons lists are distinct types with separate builtins
- CI now runs changelog fragment validation on pull requests.
- Release docs now use a fragment-first changelog workflow.

### Docs
- Added `changes/README.md` and `changes/_template.md` for contributor guidance.

---

## [v0.0.2] - 2026-01-31

### Added
- **Comparison operators**: `<=`, `>=`
- **Modulo operator**: `%`
- **Logical operators**: `&&`, `||` (short-circuiting; `&&` binds tighter than `||`)
- **Pipe operator**: `|>` — `a |> f(b)` desugars to `f(a, b)` at parse time; lowest precedence
- **Either type**: `Left(v)` / `Right(v)` keywords for explicit error handling with pattern matching
- **Lambda shorthand**: `\x -> expr` (single param) and `\(a, b) -> expr` (multiple params)
- **String interpolation**: `"Hello, #{name}!"` — any expression inside `#{ }`
- **Forward references**: functions and modules can reference names defined later in the same scope
- **Module constants**: `let` bindings inside `module { }` evaluated at compile time
- **Qualified imports with aliases**: `import Modules.Utils.Math as M`
- **Duplicate module detection**: importing same module from two roots raises `E027`
- **`--roots-only` flag**: skip default module root resolution
- **Central error code registry**: stable constants in `compiler_errors.rs`, registered in `registry.rs`
- **AST spans**: all AST nodes carry `Span { start, end }` for precise `line:col` error locations
- **Array builtins**: `concat`, `reverse`, `contains`, `slice`, `sort`
- **String builtins**: `split`, `join`, `trim`, `upper`, `lower`, `chars`, `substring`
- **Hash map builtins**: `keys`, `values`, `has_key`, `merge`, `delete`
- **Numeric builtins**: `abs`, `min`, `max`
- **Type check builtins**: `type_of`, `is_int`, `is_float`, `is_string`, `is_bool`, `is_array`, `is_hash`, `is_none`, `is_some`
- **`to_string` builtin**
- **`--trace` flag**: print VM instruction trace

### Changed
- Runtime error formatting now includes structured details, hints, and colorized code frames (`NO_COLOR=1` to disable)

### Fixed
- `split` with empty delimiter returns characters without empty ends

---

## [v0.0.1] - 2026-01-10

Initial release.

### Added
- **Bytecode compiler** — compiles Flux source to portable `.fxc` bytecode
- **Stack-based VM** — executes bytecode with call frames and closures
- **Bytecode cache** — `.fxc` files under `target/flux/`, SHA-2 hash–based invalidation
- **Immutable `let` bindings** and named `fn` functions
- **Closures** — functions capture their lexical environment
- **If / else expressions**
- **Pattern matching** — `match` on literals, `Some(x)`, `None`, wildcards
- **Module system** — `module Name { }` declarations, `import Module.Path as Alias`, `_private` prefix, forward references, import cycle detection
- **String interpolation** — `"Score: #{score}"`
- **Primitive types**: integers, floats, booleans, strings, `None`, `Some`
- **Arithmetic operators**: `+`, `-`, `*`, `/`
- **Comparison operators**: `==`, `!=`, `<`, `>`
- **Unary operators**: `-x`, `!x`
- **String concatenation**: `+`
- **Linter** — unused bindings, shadowing, unused parameters, dead code
- **Formatter** — `flux fmt [--check] <file.flx>`
- **Diagnostics** — stable error codes `E001`–`E077`, source snippets, caret highlighting, inline suggestions
- **CLI subcommands**: `tokens`, `bytecode`, `lint`, `fmt`, `cache-info`
- **`--verbose` flag**: show cache hit/miss/store status
- **Builtins**: `print`, `to_string`, `len`, `push`, `concat`, `reverse`, `contains`, `slice`, `sort`, `split`, `join`, `trim`, `upper`, `lower`, `abs`, `min`, `max`, `type_of`, `is_int`, `is_float`, `is_string`, `is_bool`, `is_array`, `is_hash`, `is_none`, `is_some`

[Unreleased]: https://github.com/sger/flux/compare/v0.0.3...HEAD
[v0.0.3]: https://github.com/sger/flux/compare/v0.0.2...v0.0.3
[v0.0.2]: https://github.com/sger/flux/compare/v0.0.1...v0.0.2
[v0.0.1]: https://github.com/sger/flux/releases/tag/v0.0.1
