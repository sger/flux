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
scripts/bench.sh core
scripts/bench.sh extended
scripts/bench.sh all
```

Run grouped flamewatch workflows:

```bash
scripts/bench_benchmark_flamewatch.sh core
scripts/bench_benchmark_flamewatch.sh extended --jit
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
scripts/bench.sh binarytrees
```

By default this uses the smaller smoke workload with `n = 8` for every language.
Flux uses `benchmarks/flux/binarytrees_smoke.flx`, and Rust/Python/Haskell are invoked with `8`.
To benchmark the aligned full baseline instead:

```bash
scripts/bench.sh binarytrees --full
```

Equivalent explicit commands:

```bash
scripts/bench.sh binarytrees \
  --flux-cmd './target/release/flux benchmarks/flux/binarytrees.flx' \
  --flux-jit-cmd './target/release/flux benchmarks/flux/binarytrees.flx --jit' \
  --rust-cmd './target/release/binarytrees_rust 21' \
  --python-cmd 'python3 benchmarks/python/binarytrees.py 21' \
  --haskell-cmd './target/release/binarytrees_hs 21' \
  --ocaml-cmd './target/release/binarytrees_ocaml 21'
```

Useful options:

```bash
scripts/bench.sh binarytrees --full --runs 3 --warmup 1
scripts/bench.sh binarytrees --runs 30 --warmup 5
scripts/bench.sh binarytrees --no-flux-jit
scripts/bench.sh binarytrees --report-file reports/binarytrees_$(date +%F).md
scripts/bench.sh binarytrees --haskell-cmd 'runghc benchmarks/haskell/binarytrees.hs'
```

Profile binarytrees specifically:

```bash
scripts/bench_benchmark_flamewatch.sh binarytrees
scripts/bench_benchmark_flamewatch.sh binarytrees --full --runs 3 --warmup 1
scripts/bench_benchmark_flamewatch.sh binarytrees --jit
```

Latest report:

<!-- binarytrees-report:start -->
# Binary Trees Benchmark Report

- Generated: 2026-03-10 10:25:21 UTC
- Runs: 10
- Warmup: 2
- Full baseline: no

| Command | Mean [ms] | Min [ms] | Max [ms] | Relative |
|:---|---:|---:|---:|---:|
| `binarytrees/flux` | 89.3 ± 30.4 | 46.5 | 147.0 | 1.69 ± 1.01 |
| `binarytrees/flux-jit` | 52.7 ± 26.0 | 29.9 | 111.0 | 1.00 |
| `binarytrees/rust` | 78.5 ± 54.2 | 4.7 | 181.3 | 1.49 ± 1.26 |
| `binarytrees/python` | 84.1 ± 21.2 | 54.3 | 115.6 | 1.60 ± 0.88 |
| `binarytrees/haskell` | 77.4 ± 39.0 | 45.8 | 165.7 | 1.47 ± 1.03 |
| `binarytrees/ocaml` | 132.9 ± 102.0 | 8.1 | 311.6 | 2.52 ± 2.30 |
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
scripts/bench.sh cfold
```

Profile the Flux implementation:

```bash
scripts/bench_benchmark_flamewatch.sh cfold
scripts/bench_benchmark_flamewatch.sh cfold --jit
```

Latest report:

<!-- cfold-report:start -->
# Constant Folding Benchmark Report

- Generated: 2026-03-10 10:23:36 UTC
- Runs: 10
- Warmup: 2

| Command | Mean [ms] | Min [ms] | Max [ms] | Relative |
|:---|---:|---:|---:|---:|
| `cfold/flux` | 74.8 ± 40.9 | 33.6 | 168.3 | 3.08 ± 1.84 |
| `cfold/flux-jit` | 69.7 ± 33.9 | 38.6 | 153.6 | 2.87 ± 1.55 |
| `cfold/rust` | 24.3 ± 5.7 | 15.6 | 32.8 | 1.00 |
| `cfold/python` | 103.3 ± 47.1 | 60.2 | 229.7 | 4.26 ± 2.18 |
| `cfold/haskell` | 36.9 ± 10.2 | 24.9 | 53.6 | 1.52 ± 0.55 |
| `cfold/ocaml` | 35.1 ± 22.2 | 16.6 | 94.4 | 1.45 ± 0.98 |
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
scripts/bench.sh deriv
```

Profile the Flux implementation:

```bash
scripts/bench_benchmark_flamewatch.sh deriv
scripts/bench_benchmark_flamewatch.sh deriv --jit
```

`deriv` currently benchmarks Flux VM by default. The Flux JIT path is not included in the cross-language matrix for this benchmark because it does not yet preserve the same counts as the VM/Haskell baseline.

Latest report:

<!-- deriv-report:start -->
# Symbolic Differentiation Benchmark Report

_Run `scripts/bench.sh deriv` to generate this report._
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
scripts/bench.sh nqueens
```

Profile the Flux implementation:

```bash
scripts/bench_benchmark_flamewatch.sh nqueens
scripts/bench_benchmark_flamewatch.sh nqueens --jit
```

Latest report:

<!-- nqueens-report:start -->
# N-Queens Benchmark Report

_Run `scripts/bench.sh nqueens` to generate this report._
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
scripts/bench.sh qsort
```

Profile the Flux implementation:

```bash
scripts/bench_benchmark_flamewatch.sh qsort
scripts/bench_benchmark_flamewatch.sh qsort --jit
```

Latest report:

<!-- qsort-report:start -->
# Quicksort Benchmark Report

_Run `scripts/bench.sh qsort` to generate this report._
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
scripts/bench.sh rbtree_ck
```

Profile the Flux implementation:

```bash
scripts/bench_benchmark_flamewatch.sh rbtree_ck
scripts/bench_benchmark_flamewatch.sh rbtree_ck --jit
```

`rbtree_ck` currently benchmarks Flux VM by default. The Flux JIT path is not included in the cross-language matrix for this benchmark unless it is explicitly re-enabled.

Latest report:

<!-- rbtree-ck-report:start -->
# Red-Black Tree Benchmark Report

_Run `scripts/bench.sh rbtree_ck` to generate this report._
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
scripts/bench.sh rbtree
```

Latest report:

<!-- rbtree-report:start -->
# Red-Black Tree Insert Benchmark Report

_Run `scripts/bench.sh rbtree` to generate this report._
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
scripts/bench.sh rbtree2
```

Latest report:

<!-- rbtree2-report:start -->
# Red-Black Tree 2 Benchmark Report

_Run `scripts/bench.sh rbtree2` to generate this report._
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
scripts/bench.sh rbtree_del
```

Latest report:

<!-- rbtree-del-report:start -->
# Red-Black Tree Delete Benchmark Report

_Run `scripts/bench.sh rbtree_del` to generate this report._
<!-- rbtree-del-report:end -->
