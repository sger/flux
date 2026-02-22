# Base Phase 3 Examples

Examples for Proposal 028 Phase 3 (`import Base except [...]`, `Base.name(...)`, and directive diagnostics).

## Valid examples

- `base_directives_ok.flx`
- `base_shadowing_ok.flx`
- `all_phase3.flx` (aggregated smoke example)

Run with VM:

```bash
cargo run -- examples/base/base_directives_ok.flx
cargo run -- examples/base/base_shadowing_ok.flx
cargo run -- examples/base/all_phase3.flx
```

Run with JIT:

```bash
cargo run --features jit -- examples/base/base_directives_ok.flx --jit
cargo run --features jit -- examples/base/base_shadowing_ok.flx --jit
cargo run --features jit -- examples/base/all_phase3.flx --jit
```

## Error examples

- `base_alias_error.flx` -> `E078`
- `base_unknown_member_error.flx` -> `E080`
- `base_duplicate_except_error.flx` -> `E079`
- `base_unknown_except_error.flx` -> `E080`

Run:

```bash
cargo run -- examples/base/base_alias_error.flx
cargo run -- examples/base/base_unknown_member_error.flx
cargo run -- examples/base/base_duplicate_except_error.flx
cargo run -- examples/base/base_unknown_except_error.flx
```
