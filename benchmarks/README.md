# Benchmarks

Benchmark sources are organized by language:

- `benchmarks/flux/`
- `benchmarks/haskell/`
- `benchmarks/ocaml/`
- `benchmarks/python/`
- `benchmarks/rust/`

## Curated Suite

The benchmark suite is split into a smaller `core` set and a broader `extended` set.

Core benchmarks:

- `binarytrees`: recursive allocation, tree traversal, and baseline VM/JIT overhead
- `cfold`: nested ADTs, recursive rewrites, and constructor-heavy control flow
- `deriv`: symbolic AST transformation and pattern-heavy recursive simplification
- `nqueens`: backtracking search and branch-heavy recursion
- `qsort`: list/array restructuring and recursive partitioning
- `rbtree_del`: balanced-tree updates, deletion, and nested constructor matching

Extended benchmarks:

- `rbtree_ck`: normalized red-black-tree construction baseline
- `rbtree`: insert-focused red-black-tree variant
- `rbtree2`: alternate red-black-tree variant with a different value shape

Run the curated groups:

```bash
scripts/bench/bench.sh core
scripts/bench/bench.sh extended
scripts/bench/bench.sh all
```

Run grouped flamewatch workflows:

```bash
scripts/bench/bench_benchmark_flamewatch.sh core
scripts/bench/bench_benchmark_flamewatch.sh extended --jit
```

Grouped flamewatch runs keep per-benchmark artifacts such as `flamegraph-cfold-vm.svg` and `flamegraph-rbtree_del-jit.svg` in the repo root.

## Binary Trees

Sources:

- `benchmarks/flux/binarytrees.flx`
- `benchmarks/flux/binarytrees_smoke.flx`
- `benchmarks/rust/binarytrees.rs`
- `benchmarks/haskell/binarytrees.hs`
- `benchmarks/ocaml/binarytrees.ml`
- `benchmarks/python/binarytrees.py`

Run each version directly:

```bash
cargo run --release --bin flux -- benchmarks/flux/binarytrees.flx
cargo run --release --features jit --bin flux -- benchmarks/flux/binarytrees.flx --jit
cargo run --release --bin binarytrees_rust
python3 benchmarks/python/binarytrees.py
runghc benchmarks/haskell/binarytrees.hs
ocaml benchmarks/ocaml/binarytrees.ml
```

For fairer Haskell and OCaml comparisons, compile them first:

```bash
ghc -O2 benchmarks/haskell/binarytrees.hs -o target/release/binarytrees_hs
ocamlopt -I +unix unix.cmxa -o target/release/binarytrees_ocaml benchmarks/ocaml/binarytrees.ml
./target/release/binarytrees_hs
./target/release/binarytrees_ocaml
```

Run the cross-language benchmark with `hyperfine`:

```bash
scripts/bench/bench.sh binarytrees
```

By default this uses the smaller smoke workload with `n = 8` for every language.
Flux uses `benchmarks/flux/binarytrees_smoke.flx`, and Rust/Python/Haskell are invoked with `8`.
To benchmark the aligned full baseline instead:

```bash
scripts/bench/bench.sh binarytrees --full
```

Equivalent explicit commands:

```bash
scripts/bench/bench.sh binarytrees \
  --flux-cmd './target/release/flux benchmarks/flux/binarytrees.flx' \
  --flux-jit-cmd './target/release/flux benchmarks/flux/binarytrees.flx --jit' \
  --rust-cmd './target/release/binarytrees_rust 21' \
  --python-cmd 'python3 benchmarks/python/binarytrees.py 21' \
  --haskell-cmd './target/release/binarytrees_hs 21' \
  --ocaml-cmd './target/release/binarytrees_ocaml 21'
```

Useful options:

```bash
scripts/bench/bench.sh binarytrees --full --runs 3 --warmup 1
scripts/bench/bench.sh binarytrees --runs 30 --warmup 5
scripts/bench/bench.sh binarytrees --no-flux-jit
scripts/bench/bench.sh binarytrees --report-file reports/binarytrees_$(date +%F).md
scripts/bench/bench.sh binarytrees --haskell-cmd 'runghc benchmarks/haskell/binarytrees.hs'
```

Profile binarytrees specifically:

```bash
scripts/bench/bench_benchmark_flamewatch.sh binarytrees
scripts/bench/bench_benchmark_flamewatch.sh binarytrees --full --runs 3 --warmup 1
scripts/bench/bench_benchmark_flamewatch.sh binarytrees --jit
```

Latest report:

<!-- binarytrees-report:start -->
# Binary Trees Benchmark Report

- Generated: 2026-03-10 15:35:32 UTC
- Runs: 10
- Warmup: 2
- Full baseline: no

| Command | Mean [ms] | Min [ms] | Max [ms] | Relative |
|:---|---:|---:|---:|---:|
| `binarytrees/flux` | 8.8 ± 0.2 | 8.5 | 9.3 | 9.62 ± 0.50 |
| `binarytrees/flux-jit` | 20.7 ± 0.4 | 20.1 | 21.3 | 22.77 ± 1.17 |
| `binarytrees/rust` | 0.9 ± 0.0 | 0.9 | 1.0 | 1.00 |
| `binarytrees/python` | 11.1 ± 0.2 | 10.9 | 11.4 | 12.16 ± 0.59 |
| `binarytrees/haskell` | 10.8 ± 0.1 | 10.7 | 10.9 | 11.91 ± 0.56 |
<!-- binarytrees-report:end -->

## Constant Folding

Sources:

- `benchmarks/flux/cfold.flx`
- `benchmarks/rust/cfold.rs`
- `benchmarks/haskell/cfold.hs`
- `benchmarks/ocaml/cfold.ml`
- `benchmarks/python/cfold.py`

Run each version directly:

```bash
cargo run --release --bin flux -- benchmarks/flux/cfold.flx
cargo run --release --features jit --bin flux -- benchmarks/flux/cfold.flx --jit
cargo run --release --bin cfold_rust
python3 benchmarks/python/cfold.py
runghc benchmarks/haskell/cfold.hs
ocaml benchmarks/ocaml/cfold.ml
```

For fairer Haskell and OCaml comparisons, compile them first:

```bash
ghc -O2 benchmarks/haskell/cfold.hs -o target/release/cfold_hs
ocamlopt -o target/release/cfold_ocaml benchmarks/ocaml/cfold.ml
./target/release/cfold_hs
./target/release/cfold_ocaml
```

Run the cross-language benchmark:

```bash
scripts/bench/bench.sh cfold
```

Profile the Flux implementation:

```bash
scripts/bench/bench_benchmark_flamewatch.sh cfold
scripts/bench/bench_benchmark_flamewatch.sh cfold --jit
```

Latest report:

<!-- cfold-report:start -->
# Constant Folding Benchmark Report

- Generated: 2026-03-10 15:36:02 UTC
- Runs: 10
- Warmup: 2

| Command | Mean [ms] | Min [ms] | Max [ms] | Relative |
|:---|---:|---:|---:|---:|
| `cfold/flux` | 13.6 ± 0.4 | 13.1 | 14.3 | 7.35 ± 0.80 |
| `cfold/flux-jit` | 32.6 ± 1.3 | 30.9 | 34.8 | 17.65 ± 2.00 |
| `cfold/rust` | 1.8 ± 0.2 | 1.6 | 2.1 | 1.00 |
| `cfold/python` | 14.3 ± 0.4 | 13.9 | 15.0 | 7.72 ± 0.84 |
| `cfold/haskell` | 10.9 ± 0.2 | 10.8 | 11.2 | 5.89 ± 0.63 |
<!-- cfold-report:end -->

## Symbolic Differentiation

Sources:

- `benchmarks/flux/deriv.flx`
- `benchmarks/rust/deriv.rs`
- `benchmarks/haskell/deriv.hs`
- `benchmarks/ocaml/deriv.ml`
- `benchmarks/python/deriv.py`

Run each version directly:

```bash
cargo run --release --bin flux -- benchmarks/flux/deriv.flx
cargo run --release --features jit --bin flux -- benchmarks/flux/deriv.flx --jit
cargo run --release --bin deriv_rust
python3 benchmarks/python/deriv.py
runghc benchmarks/haskell/deriv.hs
ocaml benchmarks/ocaml/deriv.ml
```

For fairer Haskell and OCaml comparisons, compile them first:

```bash
ghc -O2 benchmarks/haskell/deriv.hs -o target/release/deriv_hs
ocamlopt -o target/release/deriv_ocaml benchmarks/ocaml/deriv.ml
./target/release/deriv_hs
./target/release/deriv_ocaml
```

Run the cross-language benchmark:

```bash
scripts/bench/bench.sh deriv
```

Profile the Flux implementation:

```bash
scripts/bench/bench_benchmark_flamewatch.sh deriv
scripts/bench/bench_benchmark_flamewatch.sh deriv --jit
```

`deriv` currently benchmarks Flux VM by default. The Flux JIT path is not included in the cross-language matrix for this benchmark because it does not yet preserve the same counts as the VM/Haskell baseline.

Latest report:

<!-- deriv-report:start -->
# Symbolic Differentiation Benchmark Report

_Run `scripts/bench/bench.sh deriv` to generate this report._
<!-- deriv-report:end -->

## N-Queens

Sources:

- `benchmarks/flux/nqueens.flx`
- `benchmarks/rust/nqueens.rs`
- `benchmarks/haskell/nqueens.hs`
- `benchmarks/ocaml/nqueens.ml`
- `benchmarks/python/nqueens.py`

Run each version directly:

```bash
cargo run --release --bin flux -- benchmarks/flux/nqueens.flx
cargo run --release --features jit --bin flux -- benchmarks/flux/nqueens.flx --jit
cargo run --release --bin nqueens_rust
python3 benchmarks/python/nqueens.py
runghc benchmarks/haskell/nqueens.hs
ocaml benchmarks/ocaml/nqueens.ml
```

For fairer Haskell and OCaml comparisons, compile them first:

```bash
ghc -O2 benchmarks/haskell/nqueens.hs -o target/release/nqueens_hs
ocamlopt -o target/release/nqueens_ocaml benchmarks/ocaml/nqueens.ml
./target/release/nqueens_hs
./target/release/nqueens_ocaml
```

Run the cross-language benchmark:

```bash
scripts/bench/bench.sh nqueens
```

Profile the Flux implementation:

```bash
scripts/bench/bench_benchmark_flamewatch.sh nqueens
scripts/bench/bench_benchmark_flamewatch.sh nqueens --jit
```

Latest report:

<!-- nqueens-report:start -->
# N-Queens Benchmark Report

_Run `scripts/bench/bench.sh nqueens` to generate this report._
<!-- nqueens-report:end -->

## Quicksort

Sources:

- `benchmarks/flux/qsort.flx`
- `benchmarks/rust/qsort.rs`
- `benchmarks/haskell/qsort.hs`
- `benchmarks/ocaml/qsort.ml`
- `benchmarks/python/qsort.py`

Run each version directly:

```bash
cargo run --release --bin flux -- benchmarks/flux/qsort.flx
cargo run --release --features jit --bin flux -- benchmarks/flux/qsort.flx --jit
cargo run --release --bin qsort_rust
python3 benchmarks/python/qsort.py
runghc benchmarks/haskell/qsort.hs
ocaml benchmarks/ocaml/qsort.ml
```

For fairer Haskell and OCaml comparisons, compile them first:

```bash
ghc -O2 benchmarks/haskell/qsort.hs -o target/release/qsort_hs
ocamlopt -o target/release/qsort_ocaml benchmarks/ocaml/qsort.ml
./target/release/qsort_hs
./target/release/qsort_ocaml
```

Run the cross-language benchmark:

```bash
scripts/bench/bench.sh qsort
```

Profile the Flux implementation:

```bash
scripts/bench/bench_benchmark_flamewatch.sh qsort
scripts/bench/bench_benchmark_flamewatch.sh qsort --jit
```

Latest report:

<!-- qsort-report:start -->
# Quicksort Benchmark Report

- Generated: 2026-03-10 10:52:37 UTC
- Runs: 10
- Warmup: 2

| Command | Mean [ms] | Min [ms] | Max [ms] | Relative |
|:---|---:|---:|---:|---:|
| `qsort/flux` | 132.7 ± 37.3 | 92.5 | 193.1 | 2.44 ± 1.78 |
| `qsort/flux-jit` | 112.2 ± 38.9 | 85.5 | 209.7 | 2.07 ± 1.56 |
| `qsort/rust` | 169.5 ± 193.4 | 7.2 | 659.4 | 3.12 ± 4.13 |
| `qsort/python` | 191.9 ± 79.7 | 101.1 | 364.6 | 3.53 ± 2.79 |
| `qsort/haskell` | 122.0 ± 63.3 | 48.0 | 221.4 | 2.25 ± 1.91 |
| `qsort/ocaml` | 54.3 ± 36.5 | 9.1 | 127.9 | 1.00 |
<!-- qsort-report:end -->

## Red-Black Tree

Sources:

- `benchmarks/flux/rbtree_ck.flx`
- `benchmarks/rust/rbtree_ck.rs`
- `benchmarks/haskell/rbtree-ck.hs`
- `benchmarks/ocaml/rbtree_ck.ml`
- `benchmarks/python/rbtree_ck.py`

Run each version directly:

```bash
cargo run --release --bin flux -- benchmarks/flux/rbtree_ck.flx
cargo run --release --features jit --bin flux -- benchmarks/flux/rbtree_ck.flx --jit
cargo run --release --bin rbtree_ck_rust
python3 benchmarks/python/rbtree_ck.py
runghc benchmarks/haskell/rbtree-ck.hs
ocaml benchmarks/ocaml/rbtree_ck.ml
```

For fairer Haskell and OCaml comparisons, compile them first:

```bash
ghc -O2 benchmarks/haskell/rbtree-ck.hs -o target/release/rbtree_ck_hs
ocamlopt -o target/release/rbtree_ck_ocaml benchmarks/ocaml/rbtree_ck.ml
./target/release/rbtree_ck_hs
./target/release/rbtree_ck_ocaml
```

Run the cross-language benchmark:

```bash
scripts/bench/bench.sh rbtree_ck
```

Profile the Flux implementation:

```bash
scripts/bench/bench_benchmark_flamewatch.sh rbtree_ck
scripts/bench/bench_benchmark_flamewatch.sh rbtree_ck --jit
```

`rbtree_ck` currently benchmarks Flux VM by default. The Flux JIT path is not included in the cross-language matrix for this benchmark unless it is explicitly re-enabled.

Latest report:

<!-- rbtree-ck-report:start -->
# Red-Black Tree Benchmark Report

_Run `scripts/bench/bench.sh rbtree_ck` to generate this report._
<!-- rbtree-ck-report:end -->

## Red-Black Tree

Sources:

- `benchmarks/flux/rbtree.flx`
- `benchmarks/rust/rbtree.rs`
- `benchmarks/haskell/rbtree.hs`
- `benchmarks/ocaml/rbtree.ml`
- `benchmarks/python/rbtree.py`

Run the cross-language benchmark:

```bash
scripts/bench/bench.sh rbtree
```

Latest report:

<!-- rbtree-report:start -->
# Red-Black Tree Insert Benchmark Report

- Generated: 2026-03-10 13:39:20 UTC
- Runs: 10
- Warmup: 2

| Command | Mean [s] | Min [s] | Max [s] | Relative |
|:---|---:|---:|---:|---:|
| `rbtree/flux` | 4.614 ± 0.383 | 4.399 | 5.689 | 134.69 ± 21.54 |
| `rbtree/rust` | 0.145 ± 0.013 | 0.126 | 0.167 | 4.22 ± 0.68 |
| `rbtree/python` | 0.851 ± 0.080 | 0.717 | 1.032 | 24.83 ± 4.12 |
| `rbtree/haskell` | 0.056 ± 0.008 | 0.047 | 0.068 | 1.65 ± 0.32 |
| `rbtree/ocaml` | 0.034 ± 0.005 | 0.027 | 0.041 | 1.00 |
<!-- rbtree-report:end -->

## Red-Black Tree 2

Sources:

- `benchmarks/flux/rbtree2.flx`
- `benchmarks/rust/rbtree2.rs`
- `benchmarks/haskell/rbtree2.hs`
- `benchmarks/ocaml/rbtree2.ml`
- `benchmarks/python/rbtree2.py`

Run the cross-language benchmark:

```bash
scripts/bench/bench.sh rbtree2
```

Latest report:

<!-- rbtree2-report:start -->
# Red-Black Tree 2 Benchmark Report

_Run `scripts/bench/bench.sh rbtree2` to generate this report._
<!-- rbtree2-report:end -->

## Red-Black Tree Delete

Sources:

- `benchmarks/flux/rbtree_del.flx`
- `benchmarks/rust/rbtree_del.rs`
- `benchmarks/haskell/rbtree-del.hs`
- `benchmarks/ocaml/rbtree_del.ml`
- `benchmarks/python/rbtree_del.py`

Run the cross-language benchmark:

```bash
scripts/bench/bench.sh rbtree_del
```

Latest report:

<!-- rbtree-del-report:start -->
# Red-Black Tree Delete Benchmark Report

_Run `scripts/bench/bench.sh rbtree_del` to generate this report._
<!-- rbtree-del-report:end -->
