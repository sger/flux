# Flux Optimization & Analysis Guide

Quick reference for using Flux's AST optimization and analysis features.

## 🚀 Quick Start

### Run Programs

```bash
# Normal execution (fast compilation)
flux run program.flx

# Optimized execution (slower compilation, faster runtime)
flux run program.flx --optimize
flux run program.flx -O  # Short form
```

### Analyze Code

```bash
# Find undefined variables
flux analyze-free-vars program.flx

# Find tail-recursive calls
flux analyze-tail-calls program.flx

# Short aliases
flux free-vars program.flx
flux tail-calls program.flx
```

### Compare Bytecode

```bash
# View bytecode without optimization
flux bytecode program.flx

# View optimized bytecode (fewer constants, fewer instructions)
flux bytecode program.flx --optimize
```

## 📊 What Gets Optimized?

### Desugaring (Syntax Simplification)

| Input | Output | Description |
|-------|--------|-------------|
| `!!x` | `x` | Double negation elimination |
| `!!!x` | `!x` | Triple negation → single |
| `!(a == b)` | `a != b` | Negated equality → inequality |
| `!(a != b)` | `a == b` | Negated inequality → equality |

### Constant Folding (Compile-time Evaluation)

| Input | Output | Description |
|-------|--------|-------------|
| `2 + 3` | `5` | Integer arithmetic |
| `10 * 2 - 5` | `15` | Complex arithmetic |
| `"hello" + " world"` | `"hello world"` | String concatenation |
| `true && false` | `false` | Boolean logic |
| `5 > 3` | `true` | Comparisons |
| `!(2 == 2)` | `false` | After desugaring |

### Combined Example

```flux
let x = !!(2 + 3);  // Input

// Step 1: Desugar  → (2 + 3)
// Step 2: Fold     → 5
// Result: let x = 5;
```

## 📈 Performance Impact

### Example: `2 + 3 * 4`

**Without `--optimize`:**
```
Constants: [2, 3, 4]
Instructions:
  OpConstant 0  (load 2)
  OpConstant 1  (load 3)
  OpConstant 2  (load 4)
  OpMul         (3 * 4)
  OpAdd         (2 + result)
```

**With `--optimize`:**
```
Constants: [14]
Instructions:
  OpConstant 0  (load 14 directly!)
```

**Savings:**
- ✅ 67% fewer constants (3 → 1)
- ✅ 60% fewer instructions (5 → 2)
- ✅ No runtime arithmetic

## 🔍 Analysis Tools

### 1. Free Variable Analysis

**What it does:** Finds variables that are used but not defined.

**Use cases:**
- Detect typos in variable names
- Find missing imports
- Understand closure captures

**Example:**
```bash
$ flux analyze-free-vars program.flx

Free variables:
  • undefined_var
  • missing_function

Total: 2 free variable(s)
```

### 2. Tail Call Analysis

**What it does:** Finds function calls in tail position (eligible for optimization).

**Use cases:**
- Verify tail recursion is optimized
- Identify non-tail-recursive functions
- Optimize stack usage

**Example:**
```bash
$ flux analyze-tail-calls program.flx

Tail calls:
  1. Line 5: factorial(n - 1, n * acc);
  2. Line 12: process_list(rest(items));

Total: 2 tail call(s)

✓ These calls are optimized to avoid stack overflow
```

## 💡 Best Practices

### Development Mode

```bash
# Fast iteration, easier debugging
flux run program.flx
```

- ✅ Fast compilation
- ✅ Clearer error messages
- ✅ Easier to debug
- ❌ Slower runtime

### Production Mode

```bash
# Optimized for performance
flux run program.flx --optimize
```

- ✅ Faster runtime
- ✅ Smaller bytecode
- ✅ Fewer runtime operations
- ❌ Slower compilation

### Code Review

```bash
# Check for issues before committing
flux analyze-free-vars src/**/*.flx
flux analyze-tail-calls src/**/*.flx
flux lint src/**/*.flx
```

## 📦 Complete Example

### 1. Create a program

```flux
// factorial.flx
fun factorial(n, acc) {
    if n == 0 {
        acc;
    } else {
        factorial(n - 1, n * acc);  // Tail call!
    }
}

let result = !!(2 + 3);  // Will be optimized
print("Result:", result);
```

### 2. Analyze it

```bash
# Find tail calls
flux analyze-tail-calls factorial.flx
# Output: 1 tail call found (line 5)

# Check for undefined variables
flux analyze-free-vars factorial.flx
# Output: No free variables
```

### 3. Compare bytecode

```bash
# Without optimization
flux bytecode factorial.flx > unoptimized.txt

# With optimization
flux bytecode factorial.flx --optimize > optimized.txt

# Compare
diff unoptimized.txt optimized.txt
```

### 4. Run it

```bash
# Development
flux run factorial.flx

# Production
flux run factorial.flx --optimize
```

## 🎯 Common Use Cases

### Benchmarking

```bash
# Measure optimization impact
time flux run program.flx            # Baseline
time flux run program.flx --optimize # Optimized
```

### Continuous Integration

```bash
# Pre-commit checks
flux lint src/**/*.flx
flux analyze-free-vars src/**/*.flx
flux analyze-tail-calls src/**/*.flx
```

### Performance Debugging

```bash
# Why is my program slow?
flux bytecode slow_program.flx --optimize

# Are my tail calls being optimized?
flux analyze-tail-calls slow_program.flx
```

## 📚 More Information

- **examples/optimizations/** - Working examples
- **examples/optimizations/README.md** - Detailed examples guide
- **docs/architecture/** - Technical documentation

## 🔧 Troubleshooting

### "Why isn't my code optimized?"

Make sure you're using the `--optimize` flag:
```bash
flux run program.flx --optimize  # ✓ Correct
flux run program.flx             # ✗ No optimization
```

### "My tail recursion isn't working"

Check if the call is truly in tail position:
```bash
flux analyze-tail-calls program.flx
```

Non-tail example (won't optimize):
```flux
fun bad(n) {
    n * bad(n - 1);  // ✗ Multiplication happens AFTER
}
```

Tail example (will optimize):
```flux
fun good(n, acc) {
    good(n - 1, n * acc);  // ✓ Nothing after the call
}
```

### "Free variables are reported but compilation works"

Free variable analysis runs on the AST before full compilation.
The compiler will still catch undefined variables during compilation.
Use the analysis to get a complete overview before running.

## 🎉 Summary

**Optimization Flags:**
- `--optimize` or `-O` → Apply all optimizations

**Analysis Commands:**
- `analyze-free-vars` → Find undefined variables
- `analyze-tail-calls` → Find tail-recursive calls

**Comparison:**
- `bytecode` → View generated bytecode
- `bytecode --optimize` → View optimized bytecode

Happy optimizing! 🚀
