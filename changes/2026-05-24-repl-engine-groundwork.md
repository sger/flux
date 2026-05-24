### Changed
- Internal: the interactive REPL moved from `src/driver/pipeline/repl.rs` to a
  dedicated top-level `src/repl/` module, and the VM gained `run_chunk` (execute
  an additional top-level bytecode chunk on a live VM, preserving `globals`
  across chunks) plus `read_global`. This is groundwork for the
  persistent-compiler / live-VM REPL engine (proposal 0176); there is no
  user-facing change yet — the REPL still runs on the Phase 1 engine.
