# Native Async Bridge

Proposal 0174 keeps the async scheduler and backend runtime in Rust. Native
LLVM programs still link the C runtime by default; the Rust async bridge is an
optional extra archive until async operations become part of native lowering.

Build the Rust bridge archive with:

```bash
cargo build --lib
```

On Unix-like targets this produces:

```text
target/debug/libflux.a
```

Native linking can opt into that archive by setting:

```bash
FLUX_ASYNC_BRIDGE_ARCHIVE=target/debug/libflux.a
```

The hook accepts a platform path list, so later runtime splits can pass more
than one archive without changing the native linker pipeline.

Current status:

- Normal native runs do not set `FLUX_ASYNC_BRIDGE_ARCHIVE` and still link only
  `runtime/c/libflux_rt.a`.
- The exported Rust symbols are Phase 0 stubs.
- The C runtime does not own async scheduler state.
- `mio` remains a later Rust backend dependency.
