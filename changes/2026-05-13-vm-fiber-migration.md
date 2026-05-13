- Hardened VM multi-worker fiber migration: worker VMs now share Arc-backed
  read-only constants/globals, parked/yielded fibers can be stolen through the
  Arc mirror path, stolen originals are dropped on their owning worker, and
  `FLUX_FIBER_MIGRATION` now defaults on with `0`/`false`/`off` as the escape
  hatch.
- `Value::Function` and `Closure.function` now use `Arc<CompiledFunction>`, so
  shared VM state can preserve bytecode identity without racing `Rc` refcounts.
