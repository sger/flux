### Performance
- Native backend: fiber-worker OS threads are now drawn from a process-global pool instead of being spawned and joined fresh on every `run_async` / `run_async_with_workers` boundary. Sequential and nested async boundaries no longer pay per-boundary thread spawn/join churn; the pool is sized lazily to the largest `worker_count` ever requested and parked between runs. Pooled workers serve the current active `run_async`, matching the existing process-global suspension/completion routing.

### Docs
- `docs/internals/async_syntax.md` §14.8: the historical LLVM compile hang on ≥9 sequential `run_async_with*` call sites with a suspending body is no longer reproducible on LLVM 22/23 (re-verified with the original reproducer and a 40-site stress version, even with the `run_async_with*` outline workaround disabled). The outline pass is retained as a cheap safety net for older toolchains; the source-level "≤8 sites per function" guidance is obsolete.
