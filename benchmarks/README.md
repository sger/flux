# Benchmarks

Benchmark sources are organized by language:

- `benchmarks/flux/`
- `benchmarks/elixir/`
- `benchmarks/erlang/`
- `benchmarks/go/`
- `benchmarks/haskell/`
- `benchmarks/ocaml/`
- `benchmarks/python/`
- `benchmarks/rust/`

## Curated Suite

The benchmark suite is split into a smaller `core` set and a broader `extended` set.

Core benchmarks:

- `binarytrees`: recursive allocation, tree traversal, and baseline VM and native overhead
- `cfold`: nested ADTs, recursive rewrites, and constructor-heavy control flow
- `deriv`: symbolic AST transformation and pattern-heavy recursive simplification
- `nqueens`: backtracking search and branch-heavy recursion
- `qsort`: list/array restructuring and recursive partitioning
- `rbtree_del`: balanced-tree updates, deletion, and nested constructor matching
- `nbody`: float arithmetic, five planets stepped through their orbits
- `tak`: call overhead, from the Takeuchi function's deep non-tail recursion

Extended benchmarks:

- `rbtree_ck`: normalized red-black-tree construction baseline
- `rbtree`: insert-focused red-black-tree variant
- `rbtree2`: alternate red-black-tree variant with a different value shape
- `sfib`: a tail-recursive loop over 32-bit ints that wrap

The scripts need a release `flux` with the native backend:

```bash
cargo build --release --features llvm --bin flux
```

Each benchmark runs Flux twice: on the VM (`<name>/flux`), and as a native binary that
`bench.sh` emits into `target/bench/` before timing starts (`<name>/flux-native`). The
native case times only the compiled program, not the compiler.

Run the curated groups:

```bash
scripts/bench/bench.sh core
scripts/bench/bench.sh extended
scripts/bench/bench.sh all
```

The Erlang, Elixir and Go ports run when `erlc`, `elixirc` or `go` is installed; `bench.sh`
compiles them before timing starts and skips a case whose toolchain is missing. Pass an
empty command to leave one out, for example `--go-cmd ""`.

`--vms` also times two interpreters against their own native code: the Haskell program
under GHCi (`ghc -e`, which runs bytecode) and the Go program under
[yaegi](https://github.com/traefik/yaegi). yaegi must be on `PATH`:

```bash
go install github.com/traefik/yaegi/cmd/yaegi@latest
scripts/bench/bench.sh rbtree --vms
```

Both interpreters are slow on the larger workloads: yaegi takes minutes on `deriv` and
`nqueens`, and over ten on `binarytrees --full`.

Run grouped flamewatch workflows:

```bash
scripts/bench/bench_benchmark_flamewatch.sh core
scripts/bench/bench_benchmark_flamewatch.sh extended --native
```

Grouped flamewatch runs keep per-benchmark artifacts such as `flamegraph-cfold-vm.svg` and `flamegraph-rbtree_del-native.svg` in the repo root.
`--native` profiles a `-g` binary emitted into `target/bench/`, using the `flamegraph`
command that `cargo install flamegraph` provides.

## Results and baselines

Every run writes hyperfine's JSON results to `target/bench/<name>.json` (`<name>_full.json`
with `--full`). The report then compares each case with the committed baseline in
`benchmarks/baselines/` and gives the VM's time as a multiple of native's. It prints a
line instead of the table when no baseline exists yet. The comparison is informational:
it never fails a run.

Record a new baseline on a quiet machine, and commit it:

```bash
scripts/bench/bench.sh all --save-baseline
```

A baseline only compares well with runs on the machine that recorded it.

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
cargo run --release --features llvm --bin flux -- benchmarks/flux/binarytrees.flx --native
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
  --rust-cmd './target/release/binarytrees_rust 21' \
  --python-cmd 'python3 benchmarks/python/binarytrees.py 21' \
  --haskell-cmd './target/release/binarytrees_hs 21' \
  --ocaml-cmd './target/release/binarytrees_ocaml 21'
```

Useful options:

```bash
scripts/bench/bench.sh binarytrees --full --runs 3 --warmup 1
scripts/bench/bench.sh binarytrees --runs 30 --warmup 5
scripts/bench/bench.sh binarytrees --no-flux-native
scripts/bench/bench.sh binarytrees --report-file reports/binarytrees_$(date +%F).md
scripts/bench/bench.sh binarytrees --haskell-cmd 'runghc benchmarks/haskell/binarytrees.hs'
```

Profile binarytrees specifically:

```bash
scripts/bench/bench_benchmark_flamewatch.sh binarytrees
scripts/bench/bench_benchmark_flamewatch.sh binarytrees --full --runs 3 --warmup 1
scripts/bench/bench_benchmark_flamewatch.sh binarytrees --native
```

Latest report:

<!-- binarytrees-report:start -->
# Binary Trees Benchmark Report

- Generated: 2026-10-30 17:23:59 UTC
- Runs: 20
- Warmup: 2
- Full baseline: no

| Command | Mean Wall Time [ms] | Change | Factor |
|:---|---:|---:|:---|
| `binarytrees/flux` | 106.8 ± 1.2 |  |  |
| `binarytrees/flux-native` | 1.9 ± 0.1 | -98.2% | (54.8x faster) |
| `binarytrees/rust` | 1.4 ± 0.2 | -98.7% | (76.8x faster) |
| `binarytrees/python` | 9.5 ± 0.2 | -91.1% | (11.2x faster) |

### Against baseline

Baseline: `benchmarks/baselines/binarytrees.json`

| Command | Mean now | Baseline mean | Now / baseline |
|:---|---:|---:|---:|
| `binarytrees/flux` | 106.8 ms | 106.8 ms | 1.00x |
| `binarytrees/flux-native` | 1.9 ms | 1.9 ms | 1.00x |
| `binarytrees/rust` | 1.4 ms | 1.4 ms | 1.00x |
| `binarytrees/python` | 9.5 ms | 9.5 ms | 1.00x |

VM / native: 54.78x
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
cargo run --release --features llvm --bin flux -- benchmarks/flux/cfold.flx --native
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
scripts/bench/bench_benchmark_flamewatch.sh cfold --native
```

Latest report:

<!-- cfold-report:start -->
# Constant Folding Benchmark Report

- Generated: 2026-10-30 17:24:02 UTC
- Runs: 20
- Warmup: 2

| Command | Mean Wall Time [ms] | Change | Factor |
|:---|---:|---:|:---|
| `cfold/flux` | 118.5 ± 1.6 |  |  |
| `cfold/flux-native` | 2.0 ± 0.2 | -98.3% | (58.3x faster) |
| `cfold/rust` | 2.4 ± 0.2 | -98.0% | (49.7x faster) |
| `cfold/python` | 12.9 ± 0.5 | -89.1% | (9.2x faster) |

### Against baseline

Baseline: `benchmarks/baselines/cfold.json`

| Command | Mean now | Baseline mean | Now / baseline |
|:---|---:|---:|---:|
| `cfold/flux` | 118.5 ms | 118.5 ms | 1.00x |
| `cfold/flux-native` | 2.0 ms | 2.0 ms | 1.00x |
| `cfold/rust` | 2.4 ms | 2.4 ms | 1.00x |
| `cfold/python` | 12.9 ms | 12.9 ms | 1.00x |

VM / native: 58.27x
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
cargo run --release --features llvm --bin flux -- benchmarks/flux/deriv.flx --native
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
scripts/bench/bench_benchmark_flamewatch.sh deriv --native
```

Latest report:

<!-- deriv-report:start -->
# Symbolic Differentiation Benchmark Report

- Generated: 2026-10-30 17:37:45 UTC
- Runs: 3
- Warmup: 1

| Command | Mean Wall Time [min] | Change | Factor |
|:---|---:|---:|:---|
| `deriv/flux` | 1.0 ± 0.0 |  |  |
| `deriv/flux-native` | 0.0 ± 0.0 | -96.3% | (27.0x faster) |
| `deriv/rust` | 0.1 ± 0.0 | -93.6% | (15.6x faster) |
| `deriv/python` | 0.1 ± 0.0 | -88.5% | (8.7x faster) |

### Against baseline

Baseline: `benchmarks/baselines/deriv.json`

| Command | Mean now | Baseline mean | Now / baseline |
|:---|---:|---:|---:|
| `deriv/flux` | 61.970 s | 61.970 s | 1.00x |
| `deriv/flux-native` | 2.294 s | 2.294 s | 1.00x |
| `deriv/rust` | 3.985 s | 3.985 s | 1.00x |
| `deriv/python` | 7.098 s | 7.098 s | 1.00x |

VM / native: 27.02x
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
cargo run --release --features llvm --bin flux -- benchmarks/flux/nqueens.flx --native
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
scripts/bench/bench_benchmark_flamewatch.sh nqueens --native
```

Latest report:

<!-- nqueens-report:start -->
# N-Queens Benchmark Report

- Generated: 2026-10-30 17:45:53 UTC
- Runs: 3
- Warmup: 1

| Command | Mean Wall Time [min] | Change | Factor |
|:---|---:|---:|:---|
| `nqueens/flux` | 1.8 ± 0.0 |  |  |
| `nqueens/flux-native` | 0.1 ± 0.0 | -96.0% | (25.1x faster) |
| `nqueens/rust` | 0.0 ± 0.0 | -99.1% | (106.7x faster) |
| `nqueens/python` | 0.2 ± 0.0 | -90.9% | (11.0x faster) |

### Against baseline

Baseline: `benchmarks/baselines/nqueens.json`

| Command | Mean now | Baseline mean | Now / baseline |
|:---|---:|---:|---:|
| `nqueens/flux` | 106.610 s | 106.610 s | 1.00x |
| `nqueens/flux-native` | 4.246 s | 4.246 s | 1.00x |
| `nqueens/rust` | 999.3 ms | 999.3 ms | 1.00x |
| `nqueens/python` | 9.655 s | 9.655 s | 1.00x |

VM / native: 25.11x
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
cargo run --release --features llvm --bin flux -- benchmarks/flux/qsort.flx --native
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
scripts/bench/bench_benchmark_flamewatch.sh qsort --native
```

Latest report:

<!-- qsort-report:start -->
# Quicksort Benchmark Report

- Generated: 2026-10-30 17:53:55 UTC
- Runs: 20
- Warmup: 2

| Command | Mean Wall Time [ms] | Change | Factor |
|:---|---:|---:|:---|
| `qsort/flux` | 258.8 ± 3.8 |  |  |
| `qsort/flux-native` | 8.8 ± 0.5 | -96.6% | (29.4x faster) |
| `qsort/rust` | 0.7 ± 0.0 | -99.7% | (352.8x faster) |
| `qsort/python` | 12.3 ± 0.4 | -95.2% | (21.0x faster) |

### Against baseline

Baseline: `benchmarks/baselines/qsort.json`

| Command | Mean now | Baseline mean | Now / baseline |
|:---|---:|---:|---:|
| `qsort/flux` | 258.8 ms | 258.8 ms | 1.00x |
| `qsort/flux-native` | 8.8 ms | 8.8 ms | 1.00x |
| `qsort/rust` | 0.7 ms | 0.7 ms | 1.00x |
| `qsort/python` | 12.3 ms | 12.3 ms | 1.00x |

VM / native: 29.37x
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
cargo run --release --features llvm --bin flux -- benchmarks/flux/rbtree_ck.flx --native
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
scripts/bench/bench_benchmark_flamewatch.sh rbtree_ck --native
```

Latest report:

<!-- rbtree-ck-report:start -->
# Red-Black Tree Benchmark Report

- Generated: 2026-10-30 17:28:08 UTC
- Runs: 20
- Warmup: 2

| Command | Mean Wall Time [s] | Change | Factor |
|:---|---:|---:|:---|
| `rbtree_ck/flux` | 5.079 ± 0.050 |  |  |
| `rbtree_ck/flux-native` | 0.189 ± 0.001 | -96.3% | (26.9x faster) |
| `rbtree_ck/rust` | 0.046 ± 0.001 | -99.1% | (109.3x faster) |
| `rbtree_ck/python` | 0.261 ± 0.003 | -94.9% | (19.5x faster) |

### Against baseline

Baseline: `benchmarks/baselines/rbtree_ck.json`

| Command | Mean now | Baseline mean | Now / baseline |
|:---|---:|---:|---:|
| `rbtree_ck/flux` | 5.079 s | 5.079 s | 1.00x |
| `rbtree_ck/flux-native` | 188.9 ms | 188.9 ms | 1.00x |
| `rbtree_ck/rust` | 46.5 ms | 46.5 ms | 1.00x |
| `rbtree_ck/python` | 260.7 ms | 260.7 ms | 1.00x |

VM / native: 26.89x
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

- Generated: 2026-10-30 17:30:11 UTC
- Runs: 20
- Warmup: 2

| Command | Mean Wall Time [s] | Change | Factor |
|:---|---:|---:|:---|
| `rbtree/flux` | 5.078 ± 0.062 |  |  |
| `rbtree/flux-native` | 0.192 ± 0.001 | -96.2% | (26.5x faster) |
| `rbtree/rust` | 0.047 ± 0.001 | -99.1% | (109.0x faster) |
| `rbtree/python` | 0.252 ± 0.002 | -95.0% | (20.1x faster) |

### Against baseline

Baseline: `benchmarks/baselines/rbtree.json`

| Command | Mean now | Baseline mean | Now / baseline |
|:---|---:|---:|---:|
| `rbtree/flux` | 5.078 s | 5.078 s | 1.00x |
| `rbtree/flux-native` | 191.7 ms | 191.7 ms | 1.00x |
| `rbtree/rust` | 46.6 ms | 46.6 ms | 1.00x |
| `rbtree/python` | 252.5 ms | 252.5 ms | 1.00x |

VM / native: 26.49x
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
# Red-Black Tree Insert BigInt Benchmark Report

- Generated: 2026-10-30 17:32:15 UTC
- Runs: 20
- Warmup: 2

| Command | Mean Wall Time [s] | Change | Factor |
|:---|---:|---:|:---|
| `rbtree2/flux` | 5.139 ± 0.059 |  |  |
| `rbtree2/flux-native` | 0.192 ± 0.002 | -96.3% | (26.7x faster) |
| `rbtree2/rust` | 0.053 ± 0.001 | -99.0% | (96.6x faster) |
| `rbtree2/python` | 0.254 ± 0.002 | -95.1% | (20.2x faster) |

### Against baseline

Baseline: `benchmarks/baselines/rbtree2.json`

| Command | Mean now | Baseline mean | Now / baseline |
|:---|---:|---:|---:|
| `rbtree2/flux` | 5.139 s | 5.139 s | 1.00x |
| `rbtree2/flux-native` | 192.3 ms | 192.3 ms | 1.00x |
| `rbtree2/rust` | 53.2 ms | 53.2 ms | 1.00x |
| `rbtree2/python` | 254.0 ms | 254.0 ms | 1.00x |

VM / native: 26.73x
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

- Generated: 2026-10-30 17:26:05 UTC
- Runs: 20
- Warmup: 2

| Command | Mean Wall Time [s] | Change | Factor |
|:---|---:|---:|:---|
| `rbtree_del/flux` | 5.102 ± 0.042 |  |  |
| `rbtree_del/flux-native` | 0.165 ± 0.003 | -96.8% | (30.9x faster) |
| `rbtree_del/rust` | 0.048 ± 0.001 | -99.1% | (105.4x faster) |
| `rbtree_del/python` | 0.245 ± 0.002 | -95.2% | (20.9x faster) |

### Against baseline

Baseline: `benchmarks/baselines/rbtree_del.json`

| Command | Mean now | Baseline mean | Now / baseline |
|:---|---:|---:|---:|
| `rbtree_del/flux` | 5.102 s | 5.102 s | 1.00x |
| `rbtree_del/flux-native` | 164.9 ms | 164.9 ms | 1.00x |
| `rbtree_del/rust` | 48.4 ms | 48.4 ms | 1.00x |
| `rbtree_del/python` | 244.5 ms | 244.5 ms | 1.00x |

VM / native: 30.94x
<!-- rbtree-del-report:end -->

## N-Body

The Benchmarks Game's n-body: the Sun and four planets, 50,000 steps of 0.01.
Each port prints the system's energy before and after, times 10^9 and rounded,
because Flux has no way to print a float to a fixed number of decimals. At 1,000
steps the energies are the Benchmarks Game's -0.169075164 and -0.169087605.

The Flux, Haskell, OCaml, Erlang and Elixir ports build new bodies each step. Each
body adds up the pull of the others in index order, which rounds exactly as the
usual loop over pairs does, so they print the same numbers as the Rust, Go and
Python ports, which update the bodies in place.

Sources:

- `benchmarks/flux/nbody.flx`
- `benchmarks/rust/nbody.rs`
- `benchmarks/haskell/nbody.hs`
- `benchmarks/ocaml/nbody.ml`
- `benchmarks/python/nbody.py`

Run the cross-language benchmark:

```bash
scripts/bench/bench.sh nbody
```

Latest report:

<!-- nbody-report:start -->
<!-- nbody-report:end -->

## Takeuchi

`tak(30, 20, 10)`, from Koka's benchmark set. Almost all of its time goes to
calls, so it measures the cost of a call and return on each backend.

Sources:

- `benchmarks/flux/tak.flx`
- `benchmarks/rust/tak.rs`
- `benchmarks/haskell/tak.hs`
- `benchmarks/ocaml/tak.ml`
- `benchmarks/python/tak.py`

Run the cross-language benchmark:

```bash
scripts/bench/bench.sh tak
```

Latest report:

<!-- tak-report:start -->
<!-- tak-report:end -->

## Wrapping Fibonacci

Koka's `sfib`: 20,000,000 turns of a loop over two 32-bit ints, restarting when
the second passes 10^9. The ints wrap on overflow. Rust, Go and Haskell use 32-bit
types; Flux, Python, OCaml, Erlang and Elixir mask the result to 32 bits, so every
port prints the same number.

On the VM, an Int past 2^45 is stored on the heap, and `x1 * x2` often gets there,
so the VM time includes those allocations.

The native binary crashes with a stack overflow: the loop's tail call is not
compiled as a jump when an argument is `x1 * x2 + x1`. Until that is fixed, run
this benchmark with `--no-flux-native`.

Sources:

- `benchmarks/flux/sfib.flx`
- `benchmarks/rust/sfib.rs`
- `benchmarks/haskell/sfib.hs`
- `benchmarks/ocaml/sfib.ml`
- `benchmarks/python/sfib.py`

Run the cross-language benchmark:

```bash
scripts/bench/bench.sh sfib
```

Latest report:

<!-- sfib-report:start -->
<!-- sfib-report:end -->
