# Strict Types Examples

Parity tests for fully typed Flux programs (Proposal 0123, Phase 1).

Every function in these files has explicit type annotations. They serve two purposes:

1. **Parity testing**: VM vs LLVM output must match for typed programs.
2. **Strict-types validation**: All helper functions pass `--strict-types` (no `Any` in inferred types).

Note: `main()` functions currently trigger E430 under `--strict-types` because `print`/`println` return `Any`. This will be resolved when the base library is typed with type classes (Phase 3).

## Running

```bash
# Parity check (VM vs LLVM)
scripts/check_parity.sh examples/strict_types

# Strict-types validation (currently main triggers E430 due to print)
cargo run -- examples/strict_types/typed_arithmetic.flx --strict-types
```
