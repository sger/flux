# Quick Start: Optimization Examples

## 📂 Files in This Directory

1. **constant_fold_demo.flx** - Constant folding examples
2. **desugar_demo.flx** - Desugaring examples
3. **tail_recursion.flx** - Tail call optimization
4. **free_variables.flx** - Free variable detection

## 🚀 How to Run Examples

### Basic Execution

```bash
# Run constant folding demo
cargo run -- examples/optimizations/constant_fold_demo.flx

# Run with optimization
cargo run -- examples/optimizations/constant_fold_demo.flx --optimize
```

### Analyze Tail Calls

```bash
cargo run -- analyze-tail-calls examples/optimizations/tail_recursion.flx
```

**Expected Output:**
```
Tail calls in examples/optimizations/tail_recursion.flx:
──────────────────────────────────────────────────
  1. Line 9: factorial_tail(n - 1, n * acc);
  2. Line 21: fibonacci_tail(n - 1, b, a + b);

Total: 2 tail call(s)

✓ These calls are eligible for tail call optimization (TCO).
```

### Analyze Free Variables

```bash
cargo run -- analyze-free-vars examples/optimizations/free_variables.flx
```

**Expected Output:**
```
Free variables in examples/optimizations/free_variables.flx:
──────────────────────────────────────────────────
  • missing
  • undefined_var
  • unknown

Total: 3 free variable(s)
```

### Compare Bytecode

```bash
# Without optimization
cargo run -- bytecode examples/optimizations/constant_fold_demo.flx | head -30

# With optimization (see the difference!)
cargo run -- bytecode examples/optimizations/constant_fold_demo.flx --optimize | head -30
```

## 📊 Side-by-Side Comparison

### Simple Example: `2 + 3`

```bash
# Create test file
echo 'let x = 2 + 3; print(x);' > /tmp/simple.flx

# Without optimization
cargo run -- bytecode /tmp/simple.flx

# With optimization
cargo run -- bytecode /tmp/simple.flx --optimize
```

**Result:**
```
Without --optimize:           With --optimize:
━━━━━━━━━━━━━━━━━━━━━       ━━━━━━━━━━━━━━━━━━━━━
Constants:                    Constants:
  0: 2                          0: 5  ← Folded!
  1: 3
                              Instructions:
Instructions:                   OpConstant 0
  OpConstant 0                  OpSetGlobal 0
  OpConstant 1
  OpAdd        ← Runtime!    (No OpAdd needed!)
  OpSetGlobal 0
```

## 🎯 Try It Yourself

### 1. Constant Folding

```bash
# Try different expressions
echo 'print(10 * 2 + 5);' | cargo run -- -
```

Compare bytecode with and without `--optimize`

### 2. Desugaring

```bash
# Double negation
echo 'let x = !!true; print(x);' > /tmp/test.flx
cargo run -- bytecode /tmp/test.flx --optimize
# Result: No double OpBang!
```

### 3. String Concatenation

```bash
echo 'print("Hello" + " " + "World");' > /tmp/test.flx

# Without optimization: 3 constants + 2 OpAdd
cargo run -- bytecode /tmp/test.flx

# With optimization: 1 constant, no OpAdd
cargo run -- bytecode /tmp/test.flx --optimize
```

## 🔍 All Available Commands

```bash
# Execution
cargo run -- <file.flx>                    # Run program
cargo run -- <file.flx> --optimize         # Run optimized
cargo run -- <file.flx> -O                 # Short form

# Analysis
cargo run -- analyze-free-vars <file.flx>  # Find undefined vars
cargo run -- analyze-tail-calls <file.flx> # Find tail calls
cargo run -- free-vars <file.flx>          # Short alias
cargo run -- tail-calls <file.flx>         # Short alias

# Inspection
cargo run -- bytecode <file.flx>           # View bytecode
cargo run -- bytecode <file.flx> -O        # View optimized
cargo run -- lint <file.flx>               # Run linter
cargo run -- tokens <file.flx>             # View tokens
```

## 📚 More Information

- **README.md** - Detailed explanation
- **../../OPTIMIZATION_GUIDE.md** - Complete optimization guide

## 💡 Pro Tips

### Measure the Impact

```bash
# Create a computation-heavy file
cat > /tmp/heavy.flx << 'EOF'
let a = 100 * 200 + 300 * 400 - 500 / 5;
let b = (1 + 2) * (3 + 4) * (5 + 6);
let c = "foo" + "bar" + "baz" + "qux";
print(a, b, c);
EOF

# Time both versions
time cargo run -- /tmp/heavy.flx
time cargo run -- /tmp/heavy.flx --optimize
```

### Check Before Commit

```bash
# Analyze all files
cargo run -- analyze-free-vars examples/**/*.flx
cargo run -- analyze-tail-calls examples/**/*.flx
cargo run -- lint examples/**/*.flx
```

### Understand Tail Recursion

```bash
# See which calls are optimized
cargo run -- analyze-tail-calls examples/optimizations/tail_recursion.flx

# Compare with bytecode
cargo run -- bytecode examples/optimizations/tail_recursion.flx --optimize
```

Look for `OpTailCall` instructions in the bytecode!

## 🎉 Start Exploring!

Pick any example and try:
1. Run it
2. Analyze it
3. Compare bytecode
4. Modify and experiment!

Happy coding! 🚀
