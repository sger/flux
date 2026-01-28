# Memory & GC Roadmap

This document outlines pragmatic steps toward a tracing GC (Haskell‑style) for Flux.

## Why GC (vs Rc)
- Rc/Arc cannot collect cycles (leaks possible).
- Tracing GC handles cycles naturally.
- Immutability + sharing fits a generational, copying GC well.

## v0.0.x (no GC, but safer behavior)
- Keep current Rc model.
- Document that cycles can leak.
- Add debug counters for object allocation (optional).

## v0.1.x (minimal tracing GC)

### 1) Define the root set
- VM stack
- VM globals
- Call frames (closures + locals)
- Constants table

### 2) Track allocations
- All heap‑allocated Objects go through an allocator.
- Store allocations in a heap list/vector.

### 3) Mark phase
- Walk from roots and mark reachable objects.
- For composite objects (Array, Hash, Closure, Function), recurse.

### 4) Sweep phase
- Free unmarked objects.
- Clear marks for next GC cycle.

### 5) Trigger strategy
- Simple threshold: run GC after N allocations.
- Optional: run GC on cache miss / compile boundary.

## v0.2+ (generational, compacting)
- Separate young/old generations.
- Copying collection for young gen.
- Occasional full heap compaction.

## Data structures to trace
- Object::Array elements
- Object::Hash values/keys
- Object::Closure free variables
- Object::Function constants (if stored in heap)

## VM integration points
- Allocate via `vm.alloc(Object)` instead of raw clones.
- Add `vm.gc()` entrypoint.
- Debug flag: `--gc-trace` to log collection stats.

## Testing
- Unit test for cycle collection (e.g., self‑referential arrays).
- Stress test with deep recursion + large lists.
- Compare memory before/after GC.
