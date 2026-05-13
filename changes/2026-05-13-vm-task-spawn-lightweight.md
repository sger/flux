- VM `Task.spawn` now uses Arc-shared read-only constants, sparse globals
  snapshots, and thread-local pooled worker VMs instead of reconstructing a
  full isolated VM from deep-copied constants on every spawn. Public
  `Flow.Task` semantics and `Sendable` checks are unchanged.
