### Fixed
- Aether no longer transfers borrowed pattern fields into owning collection
  constructors without a `Dup`. `MakeTuple`/`MakeList`/`MakeArray`/`MakeHash`
  now demand owned arguments like ADT constructors already did, and the
  Aether-only `Dup`/`Drop` nodes survive CFG lowering instead of being erased
  by `AetherExpr::into_core()`. Fixes native memory corruption when a list of
  tuples was rebuilt from destructured fields (KI-013).
- Tuple pattern matching on the native backend now applies the same
  pointer/sentinel guard as ADT matching, so a non-pointer scrutinee fails
  loudly instead of being dereferenced (KI-013).
- A recursive rebuild reached through a borrowed argument no longer mutates a
  list the caller still holds. Borrowed call arguments carried no owning
  reference, so a callee's `IsUnique` check could observe `rc == 1` and take
  its in-place reuse arm on caller-owned data; native lowering now holds a
  temporary reference across the call (KI-018).

### Docs
- Added `docs/debugging-native-backend.md`, a repeatable process for
  investigating VM/native divergences, using KI-013 as a worked example.
