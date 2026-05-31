### Internal
- The `stdlib_list_large.flx` tail-recursion regression fixture now builds its
  large (100k) and medium (50k) input lists once at module load and shares them
  across the eight stdlib tests, instead of rebuilding them in each test. The
  functions under test (`map` / `filter` / `fold` / `take` / `take_while`) still
  traverse the full lists, so the stack-overflow coverage is unchanged, but the
  test runs ~3.8× faster (~75s → ~20s in a debug build).
