# Flux Compiler Agent Guide

## Architecture

Flux has one canonical semantic pipeline and two maintained backend families:

```text
Source
  -> syntax/        (lexer, parser, module graph)
  -> Program AST
  -> HM inference   (ast/type_infer + types/)
  -> core/          (canonical semantic IR)
  -> aether/        (Core-stage ownership/reuse transform)

VM path:
  -> cfg/           (backend-neutral CFG IR)
  -> bytecode/      (bytecode compiler + VM runtime)

Native path:
  -> lir/           (native-only low-level IR)
  -> llvm/  (LLVM IR + native compilation pipeline)
```

Keep these boundaries intact:

- `src/core/` is the only semantic IR.
- `src/aether/` is the backend-only RC lowering layer derived from Core; it is not a separate semantic IR.
- `src/cfg/` is the backend IR for the VM/default execution path.
- `src/lir/` is the native-only backend IR for the LLVM/native path.
- `src/shared_ir/` is shared ID/plumbing only, not a compiler stage.
- `structured_ir` is retired and must not be reintroduced into production paths.

## Invariants

- Do not reintroduce AST fallback into maintained backend paths.
- Do not add a second semantic IR beside `core`.
- Prefer fixing semantics in AST/Core lowering or Core passes over patching around them in backend code.
- If VM and native/LLVM differ, localize the bug to one of:
  - source program / fixture
  - syntax / module loading
  - HM inference / type-directed lowering
  - AST -> Core lowering
  - Core passes / Aether
  - Core -> CFG lowering
  - Core -> LIR lowering
  - backend/runtime execution (`bytecode` / `vm` vs `llvm`)
- Treat `--dump-core` as the first semantic debugging surface.
- Treat `--dump-aether` as the ownership/reuse debugging surface.
- Only inspect CFG/LIR/LLVM after Core and Aether look correct.

## Recommended Workflow

When changing compiler behavior:

1. Inspect the source fixture first.
2. Inspect Core with `--dump-core` or `--dump-core=debug`.
3. If ownership/reuse matters, inspect `--dump-aether`.
4. Inspect backend IR only after Core/Aether look correct:
   - VM path: CFG / bytecode / `--trace`
   - native path: `--dump-lir`, `--dump-lir-llvm`, `--emit-llvm`
5. Run the smallest relevant test or parity slice first.
6. Update docs/proposals if the architecture contract changes.

## High-Value Commands

General checks:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all --all-features
```

Compiler pipeline checks:

```bash
cargo test --test ir_pipeline_tests
cargo run -- examples/basics/arithmetic.flx --dump-core
cargo run -- examples/basics/arithmetic.flx --dump-core=debug
cargo run -- examples/basics/arithmetic.flx --dump-aether
```

Backend inspection:

```bash
cargo run -- examples/basics/arithmetic.flx --trace
cargo run --features native -- examples/basics/arithmetic.flx --native --dump-lir
cargo run --features native -- examples/basics/arithmetic.flx --native --dump-lir-llvm
cargo run --features native -- examples/basics/arithmetic.flx --native --emit-llvm
```

Parity checks:

```bash
cargo run -- parity-check tests/parity
cargo run -- parity-check examples/basics
cargo run -- parity-check tests/parity --ways vm,llvm,vm_cached,vm_strict,llvm_strict
cargo run -- parity-check examples/basics --ways vm,llvm,vm_cached,vm_strict,llvm_strict
```

## Testing Expectations

- Add focused regression tests for compiler bugs.
- Prefer snapshot/integration tests for CLI dump surfaces like `--dump-core`, `--dump-aether`, and backend dumps.
- Keep VM/native parity green for maintained suites when changing lowering, ownership, runtime, or backend execution.
- If a fixture is wrong, fix the fixture instead of normalizing around it.
- For parity regressions, prefer adding or shrinking a fixture under `tests/parity/`.

## Notes

- Use readable `--dump-core` for semantic inspection.
- Use `--dump-core=debug` when binder identity, synthetic temporaries, unresolved names, or mangled symbols matter.
- Use `--dump-aether` when borrow modes, dup/drop insertion, reuse, or FBIP/FIP behavior is in question.
- If Core already shows the wrong behavior, the bug is upstream of CFG/LIR/backend code.
- If Core matches but Aether differs, the bug is in `src/aether/` or the handoff from Core passes into Aether.
- If Core and Aether match across backends, the bug is downstream:
  - VM path: `src/cfg/`, `src/bytecode/`
  - native path: `src/lir/`, `src/llvm/`
