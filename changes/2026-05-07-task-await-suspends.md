## Changed

- Made native LLVM `Flow.Task.await` fiber-suspending instead of a blocking
  join shim. Native task workers now publish `Some(result)` / `None` task
  completions into the active async scheduler and wake the awaiting fiber.
- Kept `Task.blocking_join` as the blocking condvar path and preserved
  single-consumer task handles.
- Updated the VM `TaskAwait` path to use the same internal `Option` result
  shape while preserving sequential VM task execution.

## Tests

- Added native Flow.Task coverage for scheduler overlap, already-completed
  task await, cancellation, and unchanged blocking joins.
