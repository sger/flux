# Flow Examples

Examples for the Flow standard library (`import Flow except [...]`, `Flow.name(...)`, and directive diagnostics).

## Valid examples

- `base_directives_ok.flx`
- `base_shadowing_ok.flx`
- `all_phase3.flx` (aggregated smoke example)

Run with VM:

```bash
cargo run -- examples/flow/base_directives_ok.flx
cargo run -- examples/flow/base_shadowing_ok.flx
cargo run -- examples/flow/all_phase3.flx
```

Run with JIT:

```bash
cargo run --features jit -- examples/flow/base_directives_ok.flx --jit
cargo run --features jit -- examples/flow/base_shadowing_ok.flx --jit
cargo run --features jit -- examples/flow/all_phase3.flx --jit
```

## Error examples

- `base_alias_error.flx` -> `E078`
- `base_unknown_member_error.flx` -> `E080`
- `base_duplicate_except_error.flx` -> `E079`
- `base_unknown_except_error.flx` -> `E080`

Run:

```bash
cargo run -- examples/flow/base_alias_error.flx
cargo run -- examples/flow/base_unknown_member_error.flx
cargo run -- examples/flow/base_duplicate_except_error.flx
cargo run -- examples/flow/base_unknown_except_error.flx
```
