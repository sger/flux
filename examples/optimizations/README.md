# AST Optimization & Analysis Examples

This directory contains examples demonstrating Flux's AST optimization and analysis features.

## Files

- **constant_fold_demo.flx** - Compile-time constant folding
- **desugar_demo.flx** - Syntactic sugar elimination
- **tail_recursion.flx** - Tail call optimization
- **free_variables.flx** - Free variable detection

## Quick Start

### Run with Optimization

```bash
# Run without optimization (default)
cargo run -- examples/optimizations/constant_fold_demo.flx

# Run WITH optimization (faster bytecode)
cargo run -- examples/optimizations/constant_fold_demo.flx --optimize
cargo run -- examples/optimizations/constant_fold_demo.flx -O  # Short form
```

### Compare Bytecode

```bash
# See unoptimized bytecode
cargo run -- bytecode examples/optimizations/constant_fold_demo.flx

# See optimized bytecode (fewer constants, fewer instructions!)
cargo run -- bytecode examples/optimizations/constant_fold_demo.flx --optimize
```

### Analyze Code

```bash
# Find undefined variables
cargo run -- analyze-free-vars examples/optimizations/free_variables.flx

# Find tail calls (eligible for optimization)
cargo run -- analyze-tail-calls examples/optimizations/tail_recursion.flx
```

## Optimization Pipeline

When you use `--optimize`, Flux applies these transformations:

1. **Desugaring** - Simplify syntax
   - `!!x` → `x`
   - `!(a == b)` → `a != b`
   - `!(a != b)` → `a == b`

2. **Constant Folding** - Evaluate at compile-time
   - `2 + 3` → `5`
   - `"hello" + " world"` → `"hello world"`
   - `true && false` → `false`
   - `!(2 == 2)` → `false` (after desugaring)

3. **Analysis** - Collect information
   - Free variables (undefined identifiers)
   - Tail calls (eligible for TCO)

## Performance Impact

### Example: constant_fold_demo.flx

**Without `--optimize`:**
- Constants: 12 (includes 2, 3, "Hello", " ", "World", etc.)
- Instructions: Includes OpAdd, OpMul operations

**With `--optimize`:**
- Constants: 6 (folded to 5, "Hello World", etc.)
- Instructions: Direct values, no runtime arithmetic

**Savings:** ~50% fewer constants, faster execution!

## Detailed Examples

### 1. Constant Folding

```bash
echo 'let x = 2 + 3 * 4; print(x);' > /tmp/test.flx

# Without optimization
cargo run -- bytecode /tmp/test.flx
# Shows: Constants: 2, 3, 4 + OpMul, OpAdd instructions

# With optimization
cargo run -- bytecode /tmp/test.flx --optimize
# Shows: Constants: 14 only (pre-computed!)
```

### 2. Desugaring

```bash
echo 'let x = !!(true); let y = !(5 == 3);' > /tmp/test.flx

# Without optimization
cargo run -- bytecode /tmp/test.flx
# Shows: OpTrue, OpBang, OpBang for !!

# With optimization
cargo run -- bytecode /tmp/test.flx --optimize
# Shows: Just OpTrue (double negation eliminated!)
```

### 3. Free Variable Analysis

```bash
# Check for undefined variables
cargo run -- analyze-free-vars examples/optimizations/free_variables.flx

# Output shows which variables are used but not defined
```

### 4. Tail Call Analysis

```bash
# Find tail-recursive calls
cargo run -- analyze-tail-calls examples/optimizations/tail_recursion.flx

# Output shows which calls are in tail position (eligible for TCO)
```

## Use Cases

### Development

```bash
# Fast compilation, easier debugging
cargo run -- program.flx
```

### Production

```bash
# Optimized bytecode, better performance
cargo run -- program.flx --optimize
```

### Code Analysis

```bash
# Check for issues before running
cargo run -- analyze-free-vars program.flx
cargo run -- analyze-tail-calls program.flx
```

### Benchmarking

```bash
# Compare performance
time cargo run -- program.flx            # Baseline
time cargo run -- program.flx --optimize # Optimized
```

## See Also

- **docs/architecture/visitor_pattern_guide.md** - AST traversal patterns
