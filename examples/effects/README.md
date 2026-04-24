# Effects Examples

This directory shows the current effect system surface:

- public prelude operations such as `println`, `read_file`, and `now_ms`
- explicit function effects and aliases such as `IO` and `Time`
- modules with effectful public functions
- row-polymorphic callbacks
- user handlers
- parameterized handlers for state, reader-style environments, and captured output
- sealing
- intentional failures for missing effects, denied sealing, and reserved primop names

The user-facing operations are effect operations. Compiler-synthesized default
handlers at entrypoints delegate to internal `Flow.Primops.__primop_*`
intrinsics.
