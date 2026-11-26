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

The benchmark suite is split into a smaller `core` set, a broader `extended` set, and an
`effects` set of programs written with effect handlers.

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

Effects benchmarks, from [effect-handlers-bench](https://github.com/effect-handlers/effect-handlers-bench),
and `counter` from [Koka](https://github.com/koka-lang/koka)'s `test/bench`:

- `countdown`: a state handler's counter, read and written once per turn down to 0
- `iterator`: a range from 0 to `n`, emitted one value at a time into a handler's sum
- `counter`: a state handler counted down to 0 beside the loop's own step count
- `generator`: a tree walk's yields kept as a stream of resumes, pulled to the end
- `triples`: the decreasing triples that sum to `n`, found by a flip resumed twice at every choice
- `tree_explore`: every path of a tree explored by a choose resumed twice, under a state kept across the branches
- `fibonacci_recursive`: doubly recursive Fibonacci with no effects, the group's baseline
- `product_early`: an abort, whose clause never resumes, from 1000 frames deep
- `resume_nontail`: a clause that works on what its resume returns, the resumes nested deep
- `nqueens_choice`: backtracking through a `pick` resumed once per row, and a `fail`

Each effects program but `fibonacci_recursive` has a direct-style twin,
`benchmarks/flux/<name>_direct.flx`: the same computation without effect handlers. Their
time ratio is the effect overhead. The other languages' ports follow the twin, except
OCaml's, which use OCaml 5's effect handlers where a clause resumes at most once.

## Tools

The harness is `cargo xtask bench` (`xtask/src/bench/`). It runs the same way on Linux,
macOS and Windows. It needs [hyperfine](https://github.com/sharkdp/hyperfine). Profiling
uses [samply](https://github.com/mstange/samply), and comparing criterion runs uses
[critcmp](https://github.com/BurntSushi/critcmp). All three install with cargo:

```bash
cargo install --locked hyperfine samply critcmp
```

## Running

```bash
cargo xtask bench nqueens          # one benchmark
cargo xtask bench core             # or extended, effects, or all
```

The harness first builds a release `flux` with the native backend and the Rust ports
(`--no-build` skips this). Each benchmark runs Flux twice: on the VM (`<name>/flux`), and
as a native binary emitted with `-O` into `target/bench/` (`<name>/flux-native`). The
native case times only the compiled program, not the compiler. The Python, Haskell
(`ghc`), OCaml (`ocamlopt`), Erlang, Elixir and Go ports are compiled into `target/` when
their toolchain is on `PATH`. A case whose toolchain is missing is skipped with a note. A
port that fails to build stops the run with the compiler's error.

An effects benchmark also times its direct-style twin, on the VM (`<name>/flux-direct`)
and natively (`<name>/flux-native-direct`). The report adds a line giving the effect
overhead on each backend, the time with handlers over the time in direct style, and a
group's summary adds the two as columns.

Before timing, every case runs once and must print what the Flux VM prints. A port that
prints something else stops that benchmark before it is timed, and the run fails. Flux's
`print` shows a String in quotes, and the check ignores those quotes. `--no-check` skips
the check. The VM's check run also writes its `flux --metrics` counts.

Every program, the Flux one and each port, takes its workload size as its first
argument (Flux reads it after `--`), and without one runs at the size the suite was
written with. The harness runs each benchmark at one of two sizes, listed in
`xtask/src/bench/table.rs`, as nofib's fast and slow modes do:

- the smoke size, by default: the VM takes 0.5–2.5 s, so a whole group stays short;
- the full size, with `--full`: the native binary takes 0.1–2 s, so process startup is
  a small part of what is timed. The VM takes minutes at these sizes, and `--no-vm`
  leaves it out.

| Benchmark | Size means | Smoke | Full |
|:---|:---|---:|---:|
| `binarytrees` | maximum tree depth | 12 | 21 |
| `cfold` | expression depth | 16 | 20 |
| `deriv` | derivative steps | 8 | 10 |
| `nqueens` | board size | 11 | 13 |
| `qsort` | n rounds of sorting arrays of length 0 to n−1 | 48 | 96 |
| `rbtree_del`, `rbtree_ck`, `rbtree`, `rbtree2` | keys inserted | 50,000 | 1,000,000 |
| `nbody` | simulation steps | 20,000 | 250,000 |
| `tak` | `n` in `tak(n, 2n/3, n/3)` | 27 | 33 |
| `sfib` | loop iterations | 5,000,000 | 200,000,000 |
| `countdown` | the counter's starting value | 2,000,000 | 20,000,000 |
| `iterator` | the range's upper end | 2,000,000 | 40,000,000 |
| `counter` | the state's starting value | 2,000,000 | 20,000,000 |
| `generator` | the tree's height | 17 | 23 |
| `triples` | `n`, the largest number and the target sum | 200 | 300 |
| `tree_explore` | the tree's height | 11 | 16 |
| `fibonacci_recursive` | `n` in `fib(n)` | 33 | 42 |
| `product_early` | products of [1000, …, 0] | 2,000 | 100,000 |
| `resume_nontail` | nesting depth of the resumes, each run 1000 times | 400 | 10,000 |
| `nqueens_choice` | board size | 10 | 12 |

```bash
cargo xtask bench core --full --no-vm   # native Flux against the other languages
```

Without `--runs`, hyperfine picks between 3 and 50 runs, as many as fit in its time
budget, so a short program gets enough samples and a long one isn't run for an hour.
`--warmup` defaults to 1. Other options are `--save-baseline`, `--no-readme`,
`--report-file <path>` and `--no-native`.

`--vms` also times two interpreters against their own native code: the Haskell program
under GHCi (`ghc -e`, which runs bytecode) and the Go program under
[yaegi](https://github.com/traefik/yaegi). yaegi must be on `PATH`:

```bash
go install github.com/traefik/yaegi/cmd/yaegi@latest
cargo xtask bench rbtree --vms
```

Both interpreters are slow on the larger workloads: yaegi takes minutes on `deriv` and
`nqueens`, and over ten on `binarytrees --full`.

## Results and baselines

A run writes hyperfine's JSON to `target/bench/<name>.json` and the report to
`target/bench/<name>.md` (`<name>_full` with `--full`). The report also replaces the
benchmark's block in this file, unless `--no-readme` is given. For each case it shows
mean, standard deviation, minimum, maximum, peak memory and instructions retired, and
the time as a multiple of the native Flux binary's. hyperfine reads the instruction count
from the CPU on Linux and macOS.

Baselines are kept per kind of machine, in `benchmarks/baselines/<os>-<arch>/` (for
example `macos-aarch64`, `linux-x86_64`). Against the baseline, the report gives each
case's now / baseline ratio, the Mann–Whitney U p-value over every run's time, and the
instructions ratio, then the geometric mean of the ratios. The VM counts from
`flux --metrics` are compared with the baseline's `<name>.metrics.tsv`. The comparison
never fails a run.

How to read it:

- Instruction counts move by well under 1% between runs of the same binary, and time
  moves by several percent. A change in time with no change in instructions is usually
  the machine, not the code.
- A p below 0.05 means the change is unlikely to be noise. With fewer than 4 runs a side
  it can't get that low. Across many cases, about one in twenty unchanged ones will dip
  below 0.05 by chance.
- The VM counts repeat exactly, except `compiler_allocs`, which moves by a few dozen.

Record a baseline on a quiet machine: on mains power, with Low Power Mode or power
saving off, and with other heavy programs closed. Then commit it:

```bash
cargo xtask bench all --save-baseline
```

## Profiling

```bash
cargo xtask bench cfold --profile vm       # the VM, from a `profiling` build of flux
cargo xtask bench cfold --profile native   # a native binary built with -g
```

samply records the run, saves it to `target/bench/<name>-<way>.profile.json.gz` and
opens it in the Firefox Profiler; `samply load <file>` opens it again. The `profiling`
Cargo profile is release with line tables, so the profile names the VM's Rust functions.
The Firefox Profiler's call tree, inverted, lists the hottest functions first. Its flame
graph view replaces the SVGs `cargo flamegraph` drew.

To profile a program outside `benchmarks/`, run samply on the same build:

```bash
cargo build --profile profiling --features llvm --bin flux
samply record target/profiling/flux examples/io/aoc_day1.flx
```

## Compiler microbenchmarks

`benches/` holds criterion benchmarks of the compiler's own code (lexer, parser, Aether,
HAMT and others). Save a baseline on the base branch and one on yours, then compare
them with critcmp:

```bash
cargo bench --bench lexer_bench -- --save-baseline main
git switch my-branch
cargo bench --bench lexer_bench -- --save-baseline branch
critcmp main branch
```

## Compiler phase times

`--stats-json <file>` writes the timings `--stats` prints as JSON, one record
per phase, so two builds can be compared by a tool instead of by eye. A VM run
records `parse`, `compile` and `execute`. A native run splits compiling into
the steps `--verbose` times: `parse`, `frontend`, `llvm_modules`, `support`,
`archive`, `link`, then `execute`. The file also holds the module and size
counts `--stats` shows, with `null` for a count the backend does not have.

```bash
# The first run fills the cache; `--no-cache` neither reads nor writes it.
./target/release/flux examples/guide/arithmetic.flx --native > /dev/null
./target/release/flux examples/guide/arithmetic.flx --native --no-cache --stats-json cold.json
./target/release/flux examples/guide/arithmetic.flx --native --stats-json warm.json
cargo xtask bench stats-diff cold.json warm.json
```

`stats-diff` prints each phase's time in both files and their ratio, then
any count that changed. Times are single runs, so a small difference is noise.

## Binary Trees

Sources:

- `benchmarks/flux/binarytrees.flx`
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

Run the cross-language benchmark:

```bash
cargo xtask bench binarytrees
```

By default every language runs at depth 12; to run at depth 21 instead:

```bash
cargo xtask bench binarytrees --full
```

Useful options:

```bash
cargo xtask bench binarytrees --full --runs 3 --warmup 1
cargo xtask bench binarytrees --runs 30 --warmup 5
cargo xtask bench binarytrees --no-native
cargo xtask bench binarytrees --report-file reports/binarytrees_$(date +%F).md
```

Profile binarytrees specifically:

```bash
cargo xtask bench binarytrees --profile vm
cargo xtask bench binarytrees --full --profile native
```

Latest report:

<!-- binarytrees-report:start -->
# Binary Trees Benchmark Report

- Generated: 2026-11-25 09:48 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 12 (smoke)
- Toolchains: flux 2c0d4ca3; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `binarytrees/flux` | 454.2 ms | 1.79 ms | 450.6 ms | 455.1 ms | 21.0 MiB | 12.21 G | 134.27× |
| `binarytrees/flux-native` | 3.38 ms | 0.09 ms | 3.16 ms | 3.65 ms | 2.3 MiB | 66.3 M | 1.00× |
| `binarytrees/rust` | 17.9 ms | 1.59 ms | 16.0 ms | 24.4 ms | 2.5 MiB | 531.3 M | 5.30× |
| `binarytrees/python` | 95.7 ms | 1.10 ms | 93.8 ms | 98.7 ms | 10.6 MiB | 2.38 G | 28.29× |
| `binarytrees/erlang` | 76.6 ms | 0.81 ms | 74.8 ms | 78.1 ms | 76.6 MiB | 1.61 G | 22.65× |
| `binarytrees/elixir` | 135.2 ms | 2.78 ms | 131.9 ms | 142.5 ms | 83.1 MiB | 2.52 G | 39.98× |
| `binarytrees/go` | 7.25 ms | 0.21 ms | 6.85 ms | 8.13 ms | 10.1 MiB | 160.9 M | 2.14× |

VM / native: 134.27×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 0 | – | – |
| `aether_drops` | 50 | – | – |
| `aether_dups` | 6 | – | – |
| `aether_reuses` | 0 | – | – |
| `bytecode_bytes` | 6398 | – | – |
| `compiler_allocs` | 2032267 | – | – |
| `core_size` | 510 | – | – |
| `vm_allocs` | 12237161 | – | – |
| `vm_instructions` | 22720475 | – | – |
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
cargo xtask bench cfold
```

Profile the Flux implementation:

```bash
cargo xtask bench cfold --profile vm
cargo xtask bench cfold --profile native
```

Latest report:

<!-- cfold-report:start -->
# Constant Folding Benchmark Report

- Generated: 2026-11-25 09:49 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 16 (smoke)
- Toolchains: flux 2c0d4ca3; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `cfold/flux` | 532.3 ms | 6.57 ms | 524.5 ms | 539.5 ms | 65.0 MiB | 14.30 G | 81.39× |
| `cfold/flux-native` | 6.54 ms | 0.13 ms | 6.26 ms | 6.94 ms | 11.6 MiB | 90.8 M | 1.00× |
| `cfold/rust` | 19.0 ms | 0.76 ms | 17.9 ms | 22.3 ms | 33.4 MiB | 436.9 M | 2.91× |
| `cfold/python` | 162.3 ms | 3.39 ms | 156.0 ms | 169.1 ms | 96.0 MiB | 3.50 G | 24.83× |
| `cfold/erlang` | 88.1 ms | 2.24 ms | 85.4 ms | 99.2 ms | 121.8 MiB | 1.78 G | 13.47× |
| `cfold/elixir` | 148.6 ms | 8.04 ms | 141.6 ms | 173.6 ms | 127.0 MiB | 2.69 G | 22.72× |
| `cfold/go` | 15.7 ms | 0.52 ms | 14.7 ms | 17.0 ms | 31.9 MiB | 519.2 M | 2.41× |

VM / native: 81.39×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 0 | – | – |
| `aether_drops` | 72 | – | – |
| `aether_dups` | 28 | – | – |
| `aether_reuses` | 10 | – | – |
| `bytecode_bytes` | 6985 | – | – |
| `compiler_allocs` | 2055643 | – | – |
| `core_size` | 584 | – | – |
| `vm_allocs` | 17968490 | – | – |
| `vm_instructions` | 21058862 | – | – |
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
cargo xtask bench deriv
```

Profile the Flux implementation:

```bash
cargo xtask bench deriv --profile vm
cargo xtask bench deriv --profile native
```

Latest report:

<!-- deriv-report:start -->
# Symbolic Differentiation Benchmark Report

- Generated: 2026-11-25 09:49 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 8 (smoke)
- Toolchains: flux 2c0d4ca3; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `deriv/flux` | 1.202 s | 7.09 ms | 1.196 s | 1.210 s | 50.6 MiB | 30.99 G | 142.88× |
| `deriv/flux-native` | 8.41 ms | 0.16 ms | 8.05 ms | 8.76 ms | 12.6 MiB | 174.5 M | 1.00× |
| `deriv/rust` | 24.3 ms | 0.77 ms | 22.9 ms | 26.6 ms | 29.7 MiB | 573.5 M | 2.89× |
| `deriv/python` | 271.6 ms | 3.12 ms | 267.3 ms | 276.8 ms | 33.9 MiB | 5.70 G | 32.29× |
| `deriv/erlang` | 99.1 ms | 4.92 ms | 94.2 ms | 120.9 ms | 123.9 MiB | 1.84 G | 11.78× |
| `deriv/elixir` | 167.6 ms | 14.9 ms | 157.2 ms | 207.8 ms | 129.3 MiB | 2.77 G | 19.93× |
| `deriv/go` | 16.0 ms | 0.45 ms | 15.1 ms | 17.1 ms | 23.3 MiB | 579.6 M | 1.90× |

VM / native: 142.88×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 0 | – | – |
| `aether_drops` | 79 | – | – |
| `aether_dups` | 39 | – | – |
| `aether_reuses` | 18 | – | – |
| `bytecode_bytes` | 7904 | – | – |
| `compiler_allocs` | 2112369 | – | – |
| `core_size` | 922 | – | – |
| `vm_allocs` | 40518041 | – | – |
| `vm_instructions` | 49171565 | – | – |
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
cargo xtask bench nqueens
```

Profile the Flux implementation:

```bash
cargo xtask bench nqueens --profile vm
cargo xtask bench nqueens --profile native
```

Latest report:

<!-- nqueens-report:start -->
# N-Queens Benchmark Report

- Generated: 2026-11-25 09:50 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 11 (smoke)
- Toolchains: flux 2c0d4ca3; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `nqueens/flux` | 2.606 s | 11.9 ms | 2.592 s | 2.614 s | 19.1 MiB | 71.87 G | 182.35× |
| `nqueens/flux-native` | 14.3 ms | 0.24 ms | 13.6 ms | 14.7 ms | 2.1 MiB | 185.5 M | 1.00× |
| `nqueens/rust` | 13.8 ms | 1.62 ms | 13.2 ms | 22.5 ms | 2.0 MiB | 165.2 M | 0.97× |
| `nqueens/python` | 543.2 ms | 6.24 ms | 539.4 ms | 554.1 ms | 9.6 MiB | 14.60 G | 38.01× |
| `nqueens/erlang` | 94.0 ms | 0.83 ms | 92.4 ms | 95.4 ms | 75.7 MiB | 1.82 G | 6.58× |
| `nqueens/elixir` | 155.4 ms | 4.12 ms | 150.2 ms | 169.2 ms | 82.4 MiB | 2.72 G | 10.87× |
| `nqueens/go` | 13.1 ms | 0.38 ms | 12.8 ms | 15.6 ms | 4.5 MiB | 121.1 M | 0.92× |

VM / native: 182.35×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 0 | – | – |
| `aether_drops` | 43 | – | – |
| `aether_dups` | 6 | – | – |
| `aether_reuses` | 0 | – | – |
| `bytecode_bytes` | 6245 | – | – |
| `compiler_allocs` | 1997370 | – | – |
| `core_size` | 376 | – | – |
| `vm_allocs` | 50257371 | – | – |
| `vm_instructions` | 188827722 | – | – |
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
cargo xtask bench qsort
```

Profile the Flux implementation:

```bash
cargo xtask bench qsort --profile vm
cargo xtask bench qsort --profile native
```

Latest report:

<!-- qsort-report:start -->
# Quicksort Benchmark Report

- Generated: 2026-11-25 09:50 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 48 (smoke)
- Toolchains: flux 2c0d4ca3; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `qsort/flux` | 771.4 ms | 2.62 ms | 768.9 ms | 774.1 ms | 19.3 MiB | 20.66 G | 33.29× |
| `qsort/flux-native` | 23.2 ms | 0.56 ms | 22.2 ms | 25.8 ms | 2.2 MiB | 543.0 M | 1.00× |
| `qsort/rust` | 1.55 ms | 0.05 ms | 1.45 ms | 1.70 ms | 2.1 MiB | 18.9 M | 0.07× |
| `qsort/python` | 40.2 ms | 1.71 ms | 38.7 ms | 50.3 ms | 9.7 MiB | 780.8 M | 1.73× |
| `qsort/erlang` | 80.0 ms | 1.73 ms | 77.7 ms | 87.6 ms | 75.7 MiB | 1.56 G | 3.45× |
| `qsort/elixir` | 140.2 ms | 4.57 ms | 136.3 ms | 159.2 ms | 82.5 MiB | 2.46 G | 6.05× |
| `qsort/go` | 7.82 ms | 0.68 ms | 7.36 ms | 11.4 ms | 10.8 MiB | 135.3 M | 0.34× |

VM / native: 33.29×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 0 | – | – |
| `aether_drops` | 66 | – | – |
| `aether_dups` | 6 | – | – |
| `aether_reuses` | 0 | – | – |
| `bytecode_bytes` | 6871 | – | – |
| `compiler_allocs` | 2054042 | – | – |
| `core_size` | 727 | – | – |
| `vm_allocs` | 24707096 | – | – |
| `vm_instructions` | 32980006 | – | – |
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
cargo xtask bench rbtree_ck
```

Profile the Flux implementation:

```bash
cargo xtask bench rbtree_ck --profile vm
cargo xtask bench rbtree_ck --profile native
```

Latest report:

<!-- rbtree-ck-report:start -->
# Red-Black Tree Benchmark Report

- Generated: 2026-11-25 09:52 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 50000 (smoke)
- Toolchains: flux 2c0d4ca3; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `rbtree_ck/flux` | 2.412 s | 23.8 ms | 2.397 s | 2.439 s | 27.5 MiB | 63.69 G | 180.30× |
| `rbtree_ck/flux-native` | 13.4 ms | 0.28 ms | 13.0 ms | 14.1 ms | 4.8 MiB | 352.0 M | 1.00× |
| `rbtree_ck/rust` | 30.1 ms | 0.76 ms | 28.2 ms | 32.3 ms | 8.9 MiB | 842.6 M | 2.25× |
| `rbtree_ck/python` | 235.8 ms | 2.21 ms | 232.2 ms | 240.2 ms | 15.4 MiB | 5.79 G | 17.63× |
| `rbtree_ck/erlang` | 83.9 ms | 0.95 ms | 82.3 ms | 87.0 ms | 84.9 MiB | 1.67 G | 6.27× |
| `rbtree_ck/elixir` | 144.1 ms | 3.16 ms | 138.2 ms | 153.8 ms | 91.0 MiB | 2.56 G | 10.77× |
| `rbtree_ck/go` | 23.3 ms | 0.49 ms | 22.5 ms | 24.9 ms | 12.1 MiB | 675.3 M | 1.74× |

VM / native: 180.30×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 0 | – | – |
| `aether_drops` | 64 | – | – |
| `aether_dups` | 45 | – | – |
| `aether_reuses` | 17 | – | – |
| `bytecode_bytes` | 7831 | – | – |
| `compiler_allocs` | 2079740 | – | – |
| `core_size` | 645 | – | – |
| `vm_allocs` | 81997993 | – | – |
| `vm_instructions` | 104410666 | – | – |
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
cargo xtask bench rbtree
```

Latest report:

<!-- rbtree-report:start -->
# Red-Black Tree Insert Benchmark Report

- Generated: 2026-11-25 09:52 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 50000 (smoke)
- Toolchains: flux 2c0d4ca3; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `rbtree/flux` | 2.352 s | 17.3 ms | 2.335 s | 2.370 s | 27.4 MiB | 63.72 G | 175.43× |
| `rbtree/flux-native` | 13.4 ms | 0.80 ms | 12.7 ms | 16.9 ms | 4.8 MiB | 352.7 M | 1.00× |
| `rbtree/rust` | 34.7 ms | 1.46 ms | 29.9 ms | 37.5 ms | 8.9 MiB | 861.2 M | 2.59× |
| `rbtree/python` | 238.2 ms | 11.4 ms | 223.2 ms | 253.7 ms | 15.4 MiB | 5.58 G | 17.77× |
| `rbtree/erlang` | 97.5 ms | 10.8 ms | 86.2 ms | 127.7 ms | 85.0 MiB | 1.69 G | 7.27× |
| `rbtree/elixir` | 150.3 ms | 9.50 ms | 138.3 ms | 181.4 ms | 91.3 MiB | 2.57 G | 11.21× |
| `rbtree/go` | 22.1 ms | 0.43 ms | 21.3 ms | 24.0 ms | 12.0 MiB | 674.4 M | 1.65× |

VM / native: 175.43×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 0 | – | – |
| `aether_drops` | 65 | – | – |
| `aether_dups` | 45 | – | – |
| `aether_reuses` | 17 | – | – |
| `bytecode_bytes` | 7830 | – | – |
| `compiler_allocs` | 2080456 | – | – |
| `core_size` | 647 | – | – |
| `vm_allocs` | 81997993 | – | – |
| `vm_instructions` | 104460666 | – | – |
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
cargo xtask bench rbtree2
```

Latest report:

<!-- rbtree2-report:start -->
# Red-Black Tree Insert BigInt Benchmark Report

- Generated: 2026-11-25 09:52 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 50000 (smoke)
- Toolchains: flux 2c0d4ca3; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `rbtree2/flux` | 2.365 s | 22.8 ms | 2.348 s | 2.391 s | 27.4 MiB | 63.71 G | 177.45× |
| `rbtree2/flux-native` | 13.3 ms | 0.24 ms | 12.9 ms | 14.1 ms | 4.8 MiB | 352.0 M | 1.00× |
| `rbtree2/rust` | 34.4 ms | 1.44 ms | 31.2 ms | 37.4 ms | 12.0 MiB | 880.0 M | 2.58× |
| `rbtree2/python` | 222.9 ms | 3.35 ms | 217.6 ms | 231.5 ms | 15.4 MiB | 5.57 G | 16.72× |
| `rbtree2/erlang` | 85.0 ms | 2.63 ms | 81.1 ms | 97.9 ms | 85.0 MiB | 1.67 G | 6.38× |
| `rbtree2/elixir` | 142.8 ms | 1.08 ms | 141.5 ms | 145.6 ms | 91.1 MiB | 2.55 G | 10.71× |
| `rbtree2/go` | 23.3 ms | 0.45 ms | 22.6 ms | 24.7 ms | 12.1 MiB | 673.6 M | 1.75× |

VM / native: 177.45×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 0 | – | – |
| `aether_drops` | 64 | – | – |
| `aether_dups` | 45 | – | – |
| `aether_reuses` | 17 | – | – |
| `bytecode_bytes` | 7836 | – | – |
| `compiler_allocs` | 2081052 | – | – |
| `core_size` | 653 | – | – |
| `vm_allocs` | 81997993 | – | – |
| `vm_instructions` | 104460666 | – | – |
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
cargo xtask bench rbtree_del
```

Latest report:

<!-- rbtree-del-report:start -->
# Red-Black Tree Delete Benchmark Report

- Generated: 2026-11-25 09:50 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 50000 (smoke)
- Toolchains: flux 2c0d4ca3; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `rbtree_del/flux` | 2.464 s | 27.3 ms | 2.441 s | 2.494 s | 26.1 MiB | 64.75 G | 167.76× |
| `rbtree_del/flux-native` | 14.7 ms | 0.55 ms | 14.0 ms | 17.4 ms | 4.4 MiB | 354.1 M | 1.00× |
| `rbtree_del/rust` | 33.9 ms | 1.23 ms | 32.2 ms | 38.3 ms | 11.2 MiB | 891.8 M | 2.31× |
| `rbtree_del/python` | 224.6 ms | 2.00 ms | 221.1 ms | 227.7 ms | 14.4 MiB | 5.39 G | 15.29× |
| `rbtree_del/erlang` | 85.4 ms | 3.62 ms | 82.6 ms | 99.3 ms | 85.2 MiB | 1.67 G | 5.81× |
| `rbtree_del/elixir` | 142.4 ms | 0.84 ms | 140.5 ms | 143.6 ms | 91.3 MiB | 2.57 G | 9.69× |
| `rbtree_del/go` | 23.9 ms | 0.30 ms | 23.5 ms | 24.8 ms | 11.0 MiB | 685.0 M | 1.63× |

VM / native: 167.76×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 0 | – | – |
| `aether_drops` | 71 | – | – |
| `aether_dups` | 92 | – | – |
| `aether_reuses` | 18 | – | – |
| `bytecode_bytes` | 9570 | – | – |
| `compiler_allocs` | 2179534 | – | – |
| `core_size` | 1053 | – | – |
| `vm_allocs` | 83534581 | – | – |
| `vm_instructions` | 106005037 | – | – |
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
cargo xtask bench nbody
```

Latest report:

<!-- nbody-report:start -->
# N-Body Benchmark Report

- Generated: 2026-11-25 09:51 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 20000 (smoke)
- Toolchains: flux 2c0d4ca3; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `nbody/flux` | 1.847 s | 18.4 ms | 1.826 s | 1.859 s | 19.6 MiB | 51.17 G | 30.77× |
| `nbody/flux-native` | 60.0 ms | 1.27 ms | 57.8 ms | 63.6 ms | 2.1 MiB | 1.77 G | 1.00× |
| `nbody/rust` | 1.59 ms | 0.05 ms | 1.46 ms | 1.79 ms | 1.9 MiB | 19.2 M | 0.03× |
| `nbody/python` | 115.7 ms | 1.46 ms | 112.9 ms | 118.4 ms | 9.9 MiB | 2.85 G | 1.93× |
| `nbody/erlang` | 93.8 ms | 2.33 ms | 90.1 ms | 104.8 ms | 75.8 MiB | 1.85 G | 1.56× |
| `nbody/elixir` | 152.2 ms | 1.19 ms | 150.1 ms | 154.0 ms | 82.5 MiB | 2.75 G | 2.54× |
| `nbody/go` | 2.27 ms | 0.06 ms | 2.12 ms | 2.41 ms | 4.4 MiB | 33.4 M | 0.04× |

VM / native: 30.77×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 0 | – | – |
| `aether_drops` | 132 | – | – |
| `aether_dups` | 26 | – | – |
| `aether_reuses` | 3 | – | – |
| `bytecode_bytes` | 8106 | – | – |
| `compiler_allocs` | 2120545 | – | – |
| `core_size` | 864 | – | – |
| `vm_allocs` | 59045606 | – | – |
| `vm_instructions` | 100706154 | – | – |
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
cargo xtask bench tak
```

Latest report:

<!-- tak-report:start -->
# Takeuchi Benchmark Report

- Generated: 2026-11-25 09:51 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 27 (smoke)
- Toolchains: flux 2c0d4ca3; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `tak/flux` | 1.772 s | 13.0 ms | 1.758 s | 1.782 s | 19.1 MiB | 51.57 G | 164.00× |
| `tak/flux-native` | 10.8 ms | 0.24 ms | 10.4 ms | 11.2 ms | 2.1 MiB | 280.1 M | 1.00× |
| `tak/rust` | 9.62 ms | 0.07 ms | 9.44 ms | 9.80 ms | 2.0 MiB | 261.7 M | 0.89× |
| `tak/python` | 606.2 ms | 11.7 ms | 594.5 ms | 619.8 ms | 9.5 MiB | 16.13 G | 56.09× |
| `tak/erlang` | 94.3 ms | 5.51 ms | 89.9 ms | 120.1 ms | 75.9 MiB | 1.94 G | 8.72× |
| `tak/elixir` | 153.5 ms | 4.64 ms | 150.1 ms | 170.1 ms | 82.4 MiB | 2.83 G | 14.21× |
| `tak/go` | 12.2 ms | 0.74 ms | 11.4 ms | 13.7 ms | 4.4 MiB | 287.2 M | 1.13× |

VM / native: 164.00×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 0 | – | – |
| `aether_drops` | 36 | – | – |
| `aether_dups` | 2 | – | – |
| `aether_reuses` | 0 | – | – |
| `bytecode_bytes` | 6108 | – | – |
| `compiler_allocs` | 1985694 | – | – |
| `core_size` | 304 | – | – |
| `vm_allocs` | 31662351 | – | – |
| `vm_instructions` | 142469083 | – | – |
<!-- tak-report:end -->

## Wrapping Fibonacci

Koka's `sfib`: 20,000,000 turns of a loop over two 32-bit ints, restarting when
the second passes 10^9. The ints wrap on overflow. Rust, Go and Haskell use 32-bit
types; Flux, Python, OCaml, Erlang and Elixir mask the result to 32 bits, so every
port prints the same number.

On the VM, an Int past 2^45 is stored on the heap, and `x1 * x2` often gets there,
so the VM time includes those allocations.

The restart call passes literals to `x1` and `x2`. Both are read as Ints, so
both are owned and the call is a jump; before 17987628 they were borrowed,
and the native binary crashed with a stack overflow
([KI-148](../docs/known_issues.md#ki-148)).

Sources:

- `benchmarks/flux/sfib.flx`
- `benchmarks/rust/sfib.rs`
- `benchmarks/haskell/sfib.hs`
- `benchmarks/ocaml/sfib.ml`
- `benchmarks/python/sfib.py`

Run the cross-language benchmark:

```bash
cargo xtask bench sfib
```

Latest report:

<!-- sfib-report:start -->
# Wrapping Fibonacci Benchmark Report

- Generated: 2026-11-25 09:53 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 5000000 (smoke)
- Toolchains: flux 2c0d4ca3; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `sfib/flux` | 2.414 s | 28.7 ms | 2.381 s | 2.435 s | 19.0 MiB | 68.83 G | 377.12× |
| `sfib/flux-native` | 6.40 ms | 0.13 ms | 6.22 ms | 6.90 ms | 2.1 MiB | 104.4 M | 1.00× |
| `sfib/rust` | 7.39 ms | 0.28 ms | 7.22 ms | 8.56 ms | 2.0 MiB | 53.6 M | 1.15× |
| `sfib/python` | 622.2 ms | 3.31 ms | 619.8 ms | 627.1 ms | 9.5 MiB | 17.22 G | 97.20× |
| `sfib/erlang` | 96.9 ms | 2.91 ms | 94.1 ms | 104.7 ms | 75.7 MiB | 1.97 G | 15.13× |
| `sfib/elixir` | 162.1 ms | 6.65 ms | 154.7 ms | 179.6 ms | 82.6 MiB | 2.89 G | 25.32× |
| `sfib/go` | 4.27 ms | 0.41 ms | 3.45 ms | 5.44 ms | 4.5 MiB | 78.2 M | 0.67× |

VM / native: 377.12×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 0 | – | – |
| `aether_drops` | 36 | – | – |
| `aether_dups` | 2 | – | – |
| `aether_reuses` | 0 | – | – |
| `bytecode_bytes` | 6149 | – | – |
| `compiler_allocs` | 1986671 | – | – |
| `core_size` | 307 | – | – |
| `vm_allocs` | 27780752 | – | – |
| `vm_instructions` | 211668574 | – | – |
<!-- sfib-report:end -->

## Countdown

effect-handlers-bench's `countdown`: a parameterized `State` handler holds a counter.
The loop reads it with `get`, then writes it one lower with `set`, until it reads 0, so
each turn performs twice. Both clauses resume as their last act. The direct-style twin
passes the counter as the loop's parameter. Natively that loop compiles to almost
nothing, so the native effect overhead is mostly the cost of the handler.

The suite runs it at `n` = 200,000,000. That takes about 15 s natively, so the full size
is 20,000,000.

Sources:

- `benchmarks/flux/countdown.flx`
- `benchmarks/flux/countdown_direct.flx`
- `benchmarks/rust/countdown.rs`
- `benchmarks/go/countdown/main.go`
- `benchmarks/haskell/countdown.hs`
- `benchmarks/ocaml/countdown.ml`
- `benchmarks/python/countdown.py`
- `benchmarks/erlang/countdown.erl`
- `benchmarks/elixir/countdown.ex`

Run the cross-language benchmark:

```bash
cargo xtask bench countdown
```

Latest report:

<!-- countdown-report:start -->
<!-- countdown-report:end -->

## Iterator

effect-handlers-bench's `iterator`: the values from 0 to `n` are emitted one at a time
through an `Emit` effect, and a parameterized handler adds each into its state. Flux can't
read a handler's state after the `handle`, so a second operation, `total`, reads the sum
back: one perform more than the suite's other versions. The clauses resume as their last
act. The direct-style twin adds the range into an accumulator. The full size is the
suite's `n` = 40,000,000, whose sum is 800000020000000.

Sources:

- `benchmarks/flux/iterator.flx`
- `benchmarks/flux/iterator_direct.flx`
- `benchmarks/rust/iterator.rs`
- `benchmarks/go/iterator/main.go`
- `benchmarks/haskell/iterator.hs`
- `benchmarks/ocaml/iterator.ml`
- `benchmarks/python/iterator.py`
- `benchmarks/erlang/iterator.erl`
- `benchmarks/elixir/iterator.ex`

Run the cross-language benchmark:

```bash
cargo xtask bench iterator
```

Latest report:

<!-- iterator-report:start -->
<!-- iterator-report:end -->

## Counter

Koka's `counter` (`test/bench/koka/counter.kk`): a parameterized `State` handler holds a
value that the loop reads with `get` and writes one lower with `set`, while the loop
counts its own steps in a parameter, so it prints the starting value. It differs from
countdown in that second value, carried through the loop beside the handler's. The
direct-style twin carries both as parameters.

Koka runs it at 100,100,100. That takes about 8 s natively, so the full size is
20,000,000.

Sources:

- `benchmarks/flux/counter.flx`
- `benchmarks/flux/counter_direct.flx`
- `benchmarks/rust/counter.rs`
- `benchmarks/go/counter/main.go`
- `benchmarks/haskell/counter.hs`
- `benchmarks/ocaml/counter.ml`
- `benchmarks/python/counter.py`
- `benchmarks/erlang/counter.erl`
- `benchmarks/elixir/counter.ex`

Run the cross-language benchmark:

```bash
cargo xtask bench counter
```

Latest report:

<!-- counter-report:start -->
<!-- counter-report:end -->

## Generator

effect-handlers-bench's `generator`: a tree of height `n`, whose two subtrees are the same
tree, is walked in order, and each value is yielded through a `Yield` effect. The clause
doesn't resume. It returns the value with a thunk that calls `resume`, so the walk
becomes a stream of continuations, each resumed after its clause has returned, and `sum`
pulls it to the end. It prints 2^(n+1) − n − 2. The direct-style twin sums the tree in a
walk. OCaml's port keeps each continuation in a thunk the same way.

The suite's height of 25, whose sum is 67108837, takes about 4.4 s natively, so the full
size is 23, whose sum is 16777191.

Sources:

- `benchmarks/flux/generator.flx`
- `benchmarks/flux/generator_direct.flx`
- `benchmarks/rust/generator.rs`
- `benchmarks/go/generator/main.go`
- `benchmarks/haskell/generator.hs`
- `benchmarks/ocaml/generator.ml`
- `benchmarks/python/generator.py`
- `benchmarks/erlang/generator.erl`
- `benchmarks/elixir/generator.ex`

Run the cross-language benchmark:

```bash
cargo xtask bench generator
```

Latest report:

<!-- generator-report:start -->
# Generator Benchmark Report

- Generated: 2026-11-26 08:41 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 17 (smoke)
- Toolchains: flux c4f695dd; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; ghc 9.10.3; ocamlopt 5.5.2; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `generator/flux` | 1.152 s | 6.47 ms | 1.144 s | 1.156 s | 19.6 MiB | 31.14 G | 72.61× |
| `generator/flux-direct` | 180.4 ms | 1.88 ms | 177.4 ms | 184.0 ms | 19.3 MiB | 3.87 G | 11.37× |
| `generator/flux-native` | 15.9 ms | 0.26 ms | 15.3 ms | 16.6 ms | 2.1 MiB | 384.1 M | 1.00× |
| `generator/flux-native-direct` | 1.65 ms | 0.07 ms | 1.55 ms | 1.91 ms | 2.1 MiB | 25.7 M | 0.10× |
| `generator/rust` | 1.24 ms | 0.05 ms | 1.18 ms | 1.37 ms | 2.0 MiB | 15.4 M | 0.08× |
| `generator/python` | 23.8 ms | 0.50 ms | 22.6 ms | 24.9 ms | 9.3 MiB | 424.8 M | 1.50× |
| `generator/haskell` | 16.3 ms | 0.65 ms | 14.1 ms | 17.0 ms | 11.4 MiB | 56.2 M | 1.03× |
| `generator/ocaml` | 4.94 ms | 0.37 ms | 4.58 ms | 6.34 ms | 4.8 MiB | 44.7 M | 0.31× |
| `generator/erlang` | 73.3 ms | 1.07 ms | 71.6 ms | 75.7 ms | 61.0 MiB | 1.50 G | 4.62× |
| `generator/elixir` | 130.9 ms | 1.13 ms | 129.0 ms | 132.8 ms | 67.7 MiB | 2.41 G | 8.25× |
| `generator/go` | 1.72 ms | 0.10 ms | 1.59 ms | 2.11 ms | 4.4 MiB | 21.1 M | 0.11× |

VM / native: 72.61×

Effect overhead (with handlers / direct style): VM 6.38×, native 9.61×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 0 | – | – |
| `aether_drops` | 42 | – | – |
| `aether_dups` | 4 | – | – |
| `aether_reuses` | 0 | – | – |
| `bytecode_bytes` | 6405 | – | – |
| `compiler_allocs` | 2001192 | – | – |
| `core_size` | 311 | – | – |
| `vm_allocs` | 62458526 | – | – |
| `vm_instructions` | 11536447 | – | – |
<!-- generator-report:end -->

## Triples

effect-handlers-bench's `triples`: the strictly decreasing triples `i > j > k` that sum to
`n` are found through two effects. `choice(n)` performs `Flip.flip` and returns `n` or tries
`n - 1`, and performs `Fail.fail` when it runs out; three choices make a triple, and one with
the wrong sum fails. The `Flip` handler resumes twice, adding both branches mod 1000000007,
and the `Fail` handler, inside it, gives 0. The program prints the sum of the triples' hashes,
`(53i + 2809j + 148877k)` mod 1000000007. Flux operations aren't polymorphic, so `fail`
returns an `Int`, and the hash is taken inside the handled code rather than of a returned
tuple. The direct-style twin visits the same triples in loops. OCaml's continuations are
one-shot, so its port follows the twin too. The full size is the suite's `n` = 300, whose
answer is 460212934.

Sources:

- `benchmarks/flux/triples.flx`
- `benchmarks/flux/triples_direct.flx`
- `benchmarks/rust/triples.rs`
- `benchmarks/go/triples/main.go`
- `benchmarks/haskell/triples.hs`
- `benchmarks/ocaml/triples.ml`
- `benchmarks/python/triples.py`
- `benchmarks/erlang/triples.erl`
- `benchmarks/elixir/triples.ex`

Run the cross-language benchmark:

```bash
cargo xtask bench triples
```

Latest report:

<!-- triples-report:start -->
# Triples Benchmark Report

- Generated: 2026-11-26 09:53 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 200 (smoke)
- Toolchains: flux a7c22efa; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; ghc 9.10.3; ocamlopt 5.5.2; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `triples/flux` | 1.404 s | 6.17 ms | 1.400 s | 1.411 s | 19.3 MiB | 36.41 G | 11.58× |
| `triples/flux-direct` | 326.2 ms | 2.64 ms | 322.4 ms | 330.2 ms | 19.2 MiB | 8.47 G | 2.69× |
| `triples/flux-native` | 121.3 ms | 1.03 ms | 119.5 ms | 123.4 ms | 2.2 MiB | 3.08 G | 1.00× |
| `triples/flux-native-direct` | 1.65 ms | 0.07 ms | 1.55 ms | 1.83 ms | 2.1 MiB | 24.3 M | 0.01× |
| `triples/rust` | 1.65 ms | 0.08 ms | 1.57 ms | 1.95 ms | 2.0 MiB | 22.9 M | 0.01× |
| `triples/python` | 47.4 ms | 0.97 ms | 45.1 ms | 49.3 ms | 9.3 MiB | 1.20 G | 0.39× |
| `triples/haskell` | 16.5 ms | 0.32 ms | 14.8 ms | 16.8 ms | 11.4 MiB | 60.3 M | 0.14× |
| `triples/ocaml` | 2.23 ms | 0.24 ms | 1.98 ms | 3.15 ms | 2.6 MiB | 40.2 M | 0.02× |
| `triples/erlang` | 75.0 ms | 0.80 ms | 73.4 ms | 76.4 ms | 61.1 MiB | 1.55 G | 0.62× |
| `triples/elixir` | 133.6 ms | 2.32 ms | 131.5 ms | 143.1 ms | 67.9 MiB | 2.45 G | 1.10× |
| `triples/go` | 2.30 ms | 0.10 ms | 2.14 ms | 2.58 ms | 4.4 MiB | 37.6 M | 0.02× |

VM / native: 11.58×

Effect overhead (with handlers / direct style): VM 4.31×, native 73.38×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 0 | – | – |
| `aether_drops` | 42 | – | – |
| `aether_dups` | 3 | – | – |
| `aether_reuses` | 0 | – | – |
| `bytecode_bytes` | 6215 | – | – |
| `compiler_allocs` | 1994296 | – | – |
| `core_size` | 344 | – | – |
| `vm_allocs` | 36041570 | – | – |
| `vm_instructions` | 50774017 | – | – |
<!-- triples-report:end -->

## Tree Explore

effect-handlers-bench's `tree_explore`: every path from the root of a tree of height `n`,
whose two subtrees are the same tree, is explored by a `Choose` handler that resumes twice
and concatenates both branches' results. Before each step down, a state becomes
`operator(state, value)`, where `operator(x, y)` is `|x − 503y + 37|` mod 1009, and each path's
result is `operator` folded back up the path. The exploration runs ten times, each starting
from the previous run's maximum. Koka keeps the state in a `var`; here it is a parameterized
`St` handler around the `Choose` one. As proposal 0169 says, a handler outside the resumed
continuation keeps its state across the branches, so both give the same answers. `++` is
`concat`, and the list a path gives is built in a function of its own, `one`, since a
`handle` around `[explore(t)]` itself is rejected (KI-239). The direct-style twin threads
the state by hand and expands each choice into its two branches' results. OCaml's
continuations are one-shot, so its port follows the twin too. The full size is the suite's
height of 16, whose answer is 1005.

Sources:

- `benchmarks/flux/tree_explore.flx`
- `benchmarks/flux/tree_explore_direct.flx`
- `benchmarks/rust/tree_explore.rs`
- `benchmarks/go/tree_explore/main.go`
- `benchmarks/haskell/tree-explore.hs`
- `benchmarks/ocaml/tree_explore.ml`
- `benchmarks/python/tree_explore.py`
- `benchmarks/erlang/tree_explore.erl`
- `benchmarks/elixir/tree_explore.ex`

Run the cross-language benchmark:

```bash
cargo xtask bench tree_explore
```

Latest report:

<!-- tree-explore-report:start -->
<!-- tree-explore-report:end -->

## Recursive Fibonacci

effect-handlers-bench's `fibonacci_recursive`: `fib(42)` by double recursion, with no
effects. It measures the cost of calls in pure code, the baseline for the rest of the
effects group, so it has no direct-style twin. The suite's description gives each output
one number up, `fib(n + 1)`; its reference program, like these, starts from `fib(0) = 0`.

Sources:

- `benchmarks/flux/fibonacci_recursive.flx`
- `benchmarks/rust/fibonacci_recursive.rs`
- `benchmarks/go/fibonacci_recursive/main.go`
- `benchmarks/haskell/fibonacci-recursive.hs`
- `benchmarks/ocaml/fibonacci_recursive.ml`
- `benchmarks/python/fibonacci_recursive.py`
- `benchmarks/erlang/fibonacci_recursive.erl`
- `benchmarks/elixir/fibonacci_recursive.ex`

Run the cross-language benchmark:

```bash
cargo xtask bench fibonacci_recursive
```

Latest report:

<!-- fibonacci-recursive-report:start -->
# Recursive Fibonacci Benchmark Report

- Generated: 2026-11-25 19:24 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 33 (smoke)
- Toolchains: flux 4a439075; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; ghc 9.10.3; ocamlopt 5.5.2; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `fibonacci_recursive/flux` | 1.071 s | 17.2 ms | 1.058 s | 1.091 s | 19.1 MiB | 34.14 G | 77.71× |
| `fibonacci_recursive/flux-native` | 13.8 ms | 0.28 ms | 13.2 ms | 14.2 ms | 2.1 MiB | 214.4 M | 1.00× |
| `fibonacci_recursive/rust` | 6.17 ms | 0.22 ms | 5.79 ms | 6.49 ms | 2.0 MiB | 112.6 M | 0.45× |
| `fibonacci_recursive/python` | 463.3 ms | 3.76 ms | 458.7 ms | 470.1 ms | 9.5 MiB | 13.91 G | 33.60× |
| `fibonacci_recursive/haskell` | 21.8 ms | 6.32 ms | 15.6 ms | 28.8 ms | 11.4 MiB | 251.7 M | 1.58× |
| `fibonacci_recursive/ocaml` | 10.1 ms | 0.53 ms | 9.42 ms | 11.3 ms | 2.6 MiB | 235.6 M | 0.73× |
| `fibonacci_recursive/erlang` | 84.4 ms | 1.97 ms | 81.8 ms | 92.7 ms | 60.8 MiB | 1.83 G | 6.12× |
| `fibonacci_recursive/elixir` | 143.4 ms | 5.36 ms | 139.7 ms | 159.5 ms | 67.7 MiB | 2.73 G | 10.40× |
| `fibonacci_recursive/go` | 9.89 ms | 0.40 ms | 8.96 ms | 10.6 ms | 4.4 MiB | 207.5 M | 0.72× |

VM / native: 77.71×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 0 | – | – |
| `aether_drops` | 38 | – | – |
| `aether_dups` | 1 | – | – |
| `aether_reuses` | 0 | – | – |
| `bytecode_bytes` | 6083 | – | – |
| `compiler_allocs` | 1979086 | – | – |
| `core_size` | 251 | – | – |
| `vm_allocs` | 2977 | – | – |
| `vm_instructions` | 147444883 | – | – |
<!-- fibonacci-recursive-report:end -->

## Product with Early Exit

effect-handlers-bench's `product_early`: the product of [1000, 999, …, 0], left through
an `Abort` effect at the first zero, whose clause returns without resuming. That runs
100,000 times. The direct-style twin returns 0 at the zero and multiplies it back up
through the 1000 frames above.

Natively, each `handle` whose clause doesn't resume leaves stack behind, so about a
million turns crash ([KI-230](../docs/known_issues.md#ki-230)); the suite's 100,000 run.

Sources:

- `benchmarks/flux/product_early.flx`
- `benchmarks/flux/product_early_direct.flx`
- `benchmarks/rust/product_early.rs`
- `benchmarks/go/product_early/main.go`
- `benchmarks/haskell/product-early.hs`
- `benchmarks/ocaml/product_early.ml`
- `benchmarks/python/product_early.py`
- `benchmarks/erlang/product_early.erl`
- `benchmarks/elixir/product_early.ex`

Run the cross-language benchmark:

```bash
cargo xtask bench product_early
```

Latest report:

<!-- product-early-report:start -->
# Product with Early Exit Benchmark Report

- Generated: 2026-11-25 19:25 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 2000 (smoke)
- Toolchains: flux 4a439075; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; ghc 9.10.3; ocamlopt 5.5.2; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `product_early/flux` | 909.9 ms | 11.3 ms | 898.9 ms | 921.4 ms | 19.5 MiB | 27.58 G | 27.39× |
| `product_early/flux-direct` | 936.6 ms | 11.5 ms | 928.7 ms | 949.8 ms | 19.6 MiB | 28.82 G | 28.19× |
| `product_early/flux-native` | 33.2 ms | 0.71 ms | 32.2 ms | 35.0 ms | 2.4 MiB | 682.1 M | 1.00× |
| `product_early/flux-native-direct` | 2.87 ms | 0.49 ms | 2.42 ms | 4.50 ms | 2.2 MiB | 26.9 M | 0.09× |
| `product_early/rust` | 1.17 ms | 0.09 ms | 1.07 ms | 1.46 ms | 2.0 MiB | 13.9 M | 0.04× |
| `product_early/python` | 137.6 ms | 11.0 ms | 129.6 ms | 177.6 ms | 10.5 MiB | 2.86 G | 4.14× |
| `product_early/haskell` | 16.1 ms | 0.66 ms | 13.9 ms | 17.6 ms | 11.4 MiB | 137.5 M | 0.48× |
| `product_early/ocaml` | 6.43 ms | 0.52 ms | 5.86 ms | 8.32 ms | 44.6 MiB | 74.7 M | 0.19× |
| `product_early/erlang` | 84.8 ms | 5.27 ms | 80.4 ms | 105.7 ms | 60.9 MiB | 1.57 G | 2.55× |
| `product_early/elixir` | 142.1 ms | 3.33 ms | 138.8 ms | 154.0 ms | 67.8 MiB | 2.48 G | 4.28× |
| `product_early/go` | 7.73 ms | 0.46 ms | 7.22 ms | 9.49 ms | 4.5 MiB | 44.8 M | 0.23× |

VM / native: 27.39×

Effect overhead (with handlers / direct style): VM 0.97×, native 11.59×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 1 | – | – |
| `aether_drops` | 37 | – | – |
| `aether_dups` | 2 | – | – |
| `aether_reuses` | 0 | – | – |
| `bytecode_bytes` | 6225 | – | – |
| `compiler_allocs` | 1991749 | – | – |
| `core_size` | 310 | – | – |
| `vm_allocs` | 40074002 | – | – |
| `vm_instructions` | 62122915 | – | – |
<!-- product-early-report:end -->

## Non-Tail Resume

effect-handlers-bench's `resume_nontail`: `operator(i)` is performed for `i` from `n`
down to 1, and each clause combines its argument with what the resume returns, so the
resumes nest `n` deep. That runs 1000 times, each run starting from the last one's
result. The direct-style twin does the same combining after a call returns.

The full size is the suite's `n` = 10,000. The smoke size is 400, which keeps a VM run
to a few seconds.

Sources:

- `benchmarks/flux/resume_nontail.flx`
- `benchmarks/flux/resume_nontail_direct.flx`
- `benchmarks/rust/resume_nontail.rs`
- `benchmarks/go/resume_nontail/main.go`
- `benchmarks/haskell/resume-nontail.hs`
- `benchmarks/ocaml/resume_nontail.ml`
- `benchmarks/python/resume_nontail.py`
- `benchmarks/erlang/resume_nontail.erl`
- `benchmarks/elixir/resume_nontail.ex`

Run the cross-language benchmark:

```bash
cargo xtask bench resume_nontail
```

Latest report:

<!-- resume-nontail-report:start -->
# Non-Tail Resume Benchmark Report

- Generated: 2026-11-25 19:25 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 400 (smoke)
- Toolchains: flux 4a439075; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; ghc 9.10.3; ocamlopt 5.5.2; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `resume_nontail/flux` | 737.4 ms | 5.55 ms | 731.8 ms | 744.3 ms | 21.2 MiB | 21.66 G | 1.59× |
| `resume_nontail/flux-direct` | 310.6 ms | 3.36 ms | 306.1 ms | 316.1 ms | 19.2 MiB | 8.65 G | 0.67× |
| `resume_nontail/flux-native` | 462.5 ms | 4.73 ms | 454.0 ms | 467.8 ms | 3.9 MiB | 12.26 G | 1.00× |
| `resume_nontail/flux-native-direct` | 3.79 ms | 0.18 ms | 3.49 ms | 4.27 ms | 2.1 MiB | 32.0 M | 0.01× |
| `resume_nontail/rust` | 3.01 ms | 0.08 ms | 2.89 ms | 3.21 ms | 2.0 MiB | 24.2 M | 0.01× |
| `resume_nontail/python` | 62.7 ms | 1.49 ms | 60.7 ms | 70.3 ms | 9.8 MiB | 1.44 G | 0.14× |
| `resume_nontail/haskell` | 16.6 ms | 0.93 ms | 14.5 ms | 18.8 ms | 11.4 MiB | 62.1 M | 0.04× |
| `resume_nontail/ocaml` | 10.1 ms | 0.17 ms | 9.82 ms | 10.5 ms | 4.9 MiB | 91.4 M | 0.02× |
| `resume_nontail/erlang` | 76.3 ms | 1.15 ms | 74.1 ms | 80.5 ms | 60.8 MiB | 1.54 G | 0.17× |
| `resume_nontail/elixir` | 133.8 ms | 1.32 ms | 131.2 ms | 136.5 ms | 67.7 MiB | 2.44 G | 0.29× |
| `resume_nontail/go` | 3.71 ms | 0.14 ms | 3.43 ms | 3.97 ms | 4.5 MiB | 29.7 M | 0.01× |

VM / native: 1.59×

Effect overhead (with handlers / direct style): VM 2.37×, native 122.09×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 0 | – | – |
| `aether_drops` | 39 | – | – |
| `aether_dups` | 4 | – | – |
| `aether_reuses` | 0 | – | – |
| `bytecode_bytes` | 6255 | – | – |
| `compiler_allocs` | 1991438 | – | – |
| `core_size` | 307 | – | – |
| `vm_allocs` | 33217983 | – | – |
| `vm_instructions` | 24830895 | – | – |
<!-- resume-nontail-report:end -->

## N-Queens by Choice

effect-handlers-bench's `nqueens`, named apart from the core `nqueens`: each column's row
comes from a `pick` effect whose clause resumes once for every row, a clash `fail`s
without resuming, and the handler counts the placements that reach the last column, 14,200
on a board of 12. The direct-style twin searches the same tree with a loop over the rows.
OCaml 5's continuations resume at most once, so its port is direct style too.

Sources:

- `benchmarks/flux/nqueens_choice.flx`
- `benchmarks/flux/nqueens_choice_direct.flx`
- `benchmarks/rust/nqueens_choice.rs`
- `benchmarks/go/nqueens_choice/main.go`
- `benchmarks/haskell/nqueens-choice.hs`
- `benchmarks/ocaml/nqueens_choice.ml`
- `benchmarks/python/nqueens_choice.py`
- `benchmarks/erlang/nqueens_choice.erl`
- `benchmarks/elixir/nqueens_choice.ex`

Run the cross-language benchmark:

```bash
cargo xtask bench nqueens_choice
```

Latest report:

<!-- nqueens-choice-report:start -->
# N-Queens by Choice Benchmark Report

- Generated: 2026-11-25 19:25 UTC
- Machine: macos-aarch64
- Runs: 3 to 50, as hyperfine chose, warmup: 1
- Workload: 10 (smoke)
- Toolchains: flux 4a439075; hyperfine 2.0.0; rustc 1.100.0 (0ef1bc386 2026-11-09); Python 3.9.6; ghc 9.10.3; ocamlopt 5.5.2; erlang OTP 29; elixir 1.20.5; go1.27.4 darwin/arm64

| Case | Mean | ± σ | Min | Max | Peak RSS | Instructions | × native |
|:---|---:|---:|---:|---:|---:|---:|---:|
| `nqueens_choice/flux` | 1.293 s | 14.1 ms | 1.282 s | 1.309 s | 19.5 MiB | 37.17 G | 60.90× |
| `nqueens_choice/flux-direct` | 754.2 ms | 2.56 ms | 752.2 ms | 757.1 ms | 19.3 MiB | 22.24 G | 35.53× |
| `nqueens_choice/flux-native` | 21.2 ms | 0.27 ms | 20.7 ms | 22.0 ms | 2.1 MiB | 536.4 M | 1.00× |
| `nqueens_choice/flux-native-direct` | 3.34 ms | 0.12 ms | 3.10 ms | 3.66 ms | 2.1 MiB | 44.6 M | 0.16× |
| `nqueens_choice/rust` | 3.32 ms | 0.08 ms | 3.15 ms | 3.47 ms | 2.0 MiB | 41.9 M | 0.16× |
| `nqueens_choice/python` | 74.6 ms | 1.36 ms | 70.8 ms | 77.8 ms | 9.5 MiB | 1.89 G | 3.51× |
| `nqueens_choice/haskell` | 15.8 ms | 0.50 ms | 13.6 ms | 16.6 ms | 11.4 MiB | 102.1 M | 0.75× |
| `nqueens_choice/ocaml` | 3.84 ms | 0.11 ms | 3.60 ms | 4.14 ms | 3.4 MiB | 48.1 M | 0.18× |
| `nqueens_choice/erlang` | 76.5 ms | 1.00 ms | 74.7 ms | 78.7 ms | 60.8 MiB | 1.56 G | 3.60× |
| `nqueens_choice/elixir` | 137.0 ms | 6.73 ms | 132.1 ms | 162.0 ms | 67.5 MiB | 2.46 G | 6.45× |
| `nqueens_choice/go` | 3.76 ms | 0.14 ms | 3.43 ms | 4.09 ms | 5.0 MiB | 38.8 M | 0.18× |

VM / native: 60.90×

Effect overhead (with handlers / direct style): VM 1.71×, native 6.36×

No baseline for macos-aarch64 yet: record one with --save-baseline.

### VM counts

From `flux --metrics` on the VM case. They are the same on every run of the same compiler.

| Count | Now | Baseline | Change |
|:---|---:|---:|---:|
| `aether_allocs` | 1 | – | – |
| `aether_drops` | 42 | – | – |
| `aether_dups` | 5 | – | – |
| `aether_reuses` | 0 | – | – |
| `bytecode_bytes` | 6347 | – | – |
| `compiler_allocs` | 2003220 | – | – |
| `core_size` | 378 | – | – |
| `vm_allocs` | 48866972 | – | – |
| `vm_instructions` | 55187765 | – | – |
<!-- nqueens-choice-report:end -->
