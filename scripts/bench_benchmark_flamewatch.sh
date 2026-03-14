#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

show_help() {
  cat <<'USAGE'
Usage:
  scripts/bench_benchmark_flamewatch.sh <benchmark> [options]
  scripts/bench_benchmark_flamewatch.sh <group> [options]

Benchmarks:
  binarytrees
  cfold
  deriv
  nqueens
  qsort
  rbtree_ck
  rbtree
  rbtree2
  rbtree_del

Groups:
  core
  extended
  all

Options:
  --full                   Use the full workload when supported
  --jit                    Profile the Flux JIT path instead of the VM path
  --flux-runs <n>          Flux-only runs (default: 20)
  --flux-warmup <n>        Flux-only warmup (default: 2)
  --runs <n>               Cross-language runs (default: 10)
  --warmup <n>             Cross-language warmup (default: 2)
  --top <n>                Number of flamewatch rows to print (default: 25)
  --skip-build             Skip release build step
  --skip-flamegraph        Skip cargo flamegraph step
  -h, --help               Show this help

Examples:
  scripts/bench_benchmark_flamewatch.sh binarytrees
  scripts/bench_benchmark_flamewatch.sh core
  scripts/bench_benchmark_flamewatch.sh binarytrees --full --runs 3 --warmup 1
  scripts/bench_benchmark_flamewatch.sh cfold
  scripts/bench_benchmark_flamewatch.sh cfold --jit
  scripts/bench_benchmark_flamewatch.sh deriv
  scripts/bench_benchmark_flamewatch.sh nqueens
  scripts/bench_benchmark_flamewatch.sh qsort
  scripts/bench_benchmark_flamewatch.sh rbtree_ck
  scripts/bench_benchmark_flamewatch.sh rbtree
  scripts/bench_benchmark_flamewatch.sh rbtree2
  scripts/bench_benchmark_flamewatch.sh rbtree_del
USAGE
}

CORE_BENCHMARKS=(binarytrees cfold deriv nqueens qsort rbtree_del)
EXTENDED_BENCHMARKS=(rbtree_ck rbtree rbtree2)

is_group() {
  case "$1" in
    core|extended|all) return 0 ;;
    *) return 1 ;;
  esac
}

group_members() {
  case "$1" in
    core) printf '%s\n' "${CORE_BENCHMARKS[@]}" ;;
    extended) printf '%s\n' "${EXTENDED_BENCHMARKS[@]}" ;;
    all) printf '%s\n' "${CORE_BENCHMARKS[@]}" "${EXTENDED_BENCHMARKS[@]}" ;;
  esac
}

REQUESTED_BENCHMARK="${1:-}"
if [[ -z "$REQUESTED_BENCHMARK" || "$REQUESTED_BENCHMARK" == -* ]]; then
  show_help >&2
  exit 1
fi
shift
declare -a PASSTHROUGH_ARGS=("$@")

if is_group "$REQUESTED_BENCHMARK"; then
  for arg in "${PASSTHROUGH_ARGS[@]-}"; do
    case "$arg" in
      -h|--help)
        show_help
        exit 0
        ;;
    esac
  done
  while IFS= read -r benchmark; do
    [[ -n "$benchmark" ]] || continue
    echo "== ${REQUESTED_BENCHMARK}: ${benchmark} =="
    child_cmd=("$0" "$benchmark")
    if [[ ${#PASSTHROUGH_ARGS[@]} -gt 0 ]]; then
      child_cmd+=("${PASSTHROUGH_ARGS[@]}")
    fi
    "${child_cmd[@]}"
    echo
  done < <(group_members "$REQUESTED_BENCHMARK")
  exit 0
fi

BENCHMARK="$REQUESTED_BENCHMARK"

FULL_MODE=0
FLUX_RUNS=20
FLUX_WARMUP=2
RUNS=10
WARMUP=2
TOP_N=25
SKIP_BUILD=0
SKIP_FLAMEGRAPH=0
PROFILE_JIT=0
FLAMEGRAPH_ARTIFACT=""
TRACE_ARTIFACT=""

SUPPORTS_FULL=0
FLUX_PROGRAM_SMOKE=""
FLUX_PROGRAM_FULL=""
PROFILE_PROGRAM=""
RUST_BIN=""
HASKELL_SOURCE=""
HASKELL_BIN=""
OCAML_SOURCE=""
OCAML_BIN=""

configure_benchmark() {
  case "$BENCHMARK" in
    binarytrees)
      SUPPORTS_FULL=1
      FLUX_PROGRAM_SMOKE="benchmarks/flux/binarytrees_smoke.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/binarytrees.flx"
      PROFILE_PROGRAM="benchmarks/flux/binarytrees.flx"
      RUST_BIN="binarytrees_rust"
      HASKELL_SOURCE="benchmarks/haskell/binarytrees.hs"
      HASKELL_BIN="target/release/binarytrees_hs"
      OCAML_SOURCE="benchmarks/ocaml/binarytrees.ml"
      OCAML_BIN="target/release/binarytrees_ocaml"
      ;;
    cfold)
      FLUX_PROGRAM_SMOKE="benchmarks/flux/cfold.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/cfold.flx"
      PROFILE_PROGRAM="benchmarks/flux/cfold.flx"
      RUST_BIN="cfold_rust"
      HASKELL_SOURCE="benchmarks/haskell/cfold.hs"
      HASKELL_BIN="target/release/cfold_hs"
      OCAML_SOURCE="benchmarks/ocaml/cfold.ml"
      OCAML_BIN="target/release/cfold_ocaml"
      ;;
    deriv)
      FLUX_PROGRAM_SMOKE="benchmarks/flux/deriv.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/deriv.flx"
      PROFILE_PROGRAM="benchmarks/flux/deriv.flx"
      RUST_BIN="deriv_rust"
      HASKELL_SOURCE="benchmarks/haskell/deriv.hs"
      HASKELL_BIN="target/release/deriv_hs"
      OCAML_SOURCE="benchmarks/ocaml/deriv.ml"
      OCAML_BIN="target/release/deriv_ocaml"
      ;;
    nqueens)
      FLUX_PROGRAM_SMOKE="benchmarks/flux/nqueens.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/nqueens.flx"
      PROFILE_PROGRAM="benchmarks/flux/nqueens.flx"
      RUST_BIN="nqueens_rust"
      HASKELL_SOURCE="benchmarks/haskell/nqueens.hs"
      HASKELL_BIN="target/release/nqueens_hs"
      OCAML_SOURCE="benchmarks/ocaml/nqueens.ml"
      OCAML_BIN="target/release/nqueens_ocaml"
      ;;
    qsort)
      FLUX_PROGRAM_SMOKE="benchmarks/flux/qsort.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/qsort.flx"
      PROFILE_PROGRAM="benchmarks/flux/qsort.flx"
      RUST_BIN="qsort_rust"
      HASKELL_SOURCE="benchmarks/haskell/qsort.hs"
      HASKELL_BIN="target/release/qsort_hs"
      OCAML_SOURCE="benchmarks/ocaml/qsort.ml"
      OCAML_BIN="target/release/qsort_ocaml"
      ;;
    rbtree_ck)
      FLUX_PROGRAM_SMOKE="benchmarks/flux/rbtree_ck.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/rbtree_ck.flx"
      PROFILE_PROGRAM="benchmarks/flux/rbtree_ck.flx"
      RUST_BIN="rbtree_ck_rust"
      HASKELL_SOURCE="benchmarks/haskell/rbtree-ck.hs"
      HASKELL_BIN="target/release/rbtree_ck_hs"
      OCAML_SOURCE="benchmarks/ocaml/rbtree_ck.ml"
      OCAML_BIN="target/release/rbtree_ck_ocaml"
      ;;
    rbtree)
      FLUX_PROGRAM_SMOKE="benchmarks/flux/rbtree.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/rbtree.flx"
      PROFILE_PROGRAM="benchmarks/flux/rbtree.flx"
      RUST_BIN="rbtree_rust"
      HASKELL_SOURCE="benchmarks/haskell/rbtree.hs"
      HASKELL_BIN="target/release/rbtree_hs"
      OCAML_SOURCE="benchmarks/ocaml/rbtree.ml"
      OCAML_BIN="target/release/rbtree_ocaml"
      ;;
    rbtree2)
      FLUX_PROGRAM_SMOKE="benchmarks/flux/rbtree2.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/rbtree2.flx"
      PROFILE_PROGRAM="benchmarks/flux/rbtree2.flx"
      RUST_BIN="rbtree2_rust"
      HASKELL_SOURCE="benchmarks/haskell/rbtree2.hs"
      HASKELL_BIN="target/release/rbtree2_hs"
      OCAML_SOURCE="benchmarks/ocaml/rbtree2.ml"
      OCAML_BIN="target/release/rbtree2_ocaml"
      ;;
    rbtree_del)
      FLUX_PROGRAM_SMOKE="benchmarks/flux/rbtree_del.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/rbtree_del.flx"
      PROFILE_PROGRAM="benchmarks/flux/rbtree_del.flx"
      RUST_BIN="rbtree_del_rust"
      HASKELL_SOURCE="benchmarks/haskell/rbtree-del.hs"
      HASKELL_BIN="target/release/rbtree_del_hs"
      OCAML_SOURCE="benchmarks/ocaml/rbtree_del.ml"
      OCAML_BIN="target/release/rbtree_del_ocaml"
      ;;
    *)
      echo "Error: unknown benchmark '$BENCHMARK'." >&2
      show_help >&2
      exit 1
      ;;
  esac
}

configure_benchmark

while [[ $# -gt 0 ]]; do
  case "$1" in
    --full)
      FULL_MODE=1
      shift
      ;;
    --jit)
      PROFILE_JIT=1
      shift
      ;;
    --flux-runs)
      FLUX_RUNS="$2"
      shift 2
      ;;
    --flux-warmup)
      FLUX_WARMUP="$2"
      shift 2
      ;;
    --runs)
      RUNS="$2"
      shift 2
      ;;
    --warmup)
      WARMUP="$2"
      shift 2
      ;;
    --top)
      TOP_N="$2"
      shift 2
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --skip-flamegraph)
      SKIP_FLAMEGRAPH=1
      shift
      ;;
    -h|--help)
      show_help
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      show_help >&2
      exit 1
      ;;
  esac
done

if [[ "$FULL_MODE" -eq 1 && "$SUPPORTS_FULL" -ne 1 ]]; then
  echo "Error: benchmark '$BENCHMARK' does not define a separate full workload." >&2
  exit 1
fi

if ! command -v hyperfine >/dev/null 2>&1; then
  echo "Error: hyperfine not found." >&2
  exit 1
fi

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  echo "== Build release binaries =="
  cargo build --release --features jit --bin flux --bin "$RUST_BIN"
  if [[ -n "$HASKELL_SOURCE" ]]; then
    if command -v ghc >/dev/null 2>&1; then
      ghc -O2 "$HASKELL_SOURCE" -o "$HASKELL_BIN"
    else
      echo "Warning: ghc not found; Haskell benchmark binary will not be rebuilt." >&2
    fi
  fi
  if [[ -n "$OCAML_SOURCE" ]]; then
    if command -v ocamlopt >/dev/null 2>&1; then
      if [[ "$BENCHMARK" == "binarytrees" ]]; then
        ocamlopt -I +unix unix.cmxa -o "$OCAML_BIN" "$OCAML_SOURCE"
      else
        ocamlopt -o "$OCAML_BIN" "$OCAML_SOURCE"
      fi
    else
      echo "Warning: ocamlopt not found; OCaml benchmark binary will not be rebuilt." >&2
    fi
  fi
  echo
fi

if [[ "$FULL_MODE" -eq 1 ]]; then
  FLUX_PROGRAM="$FLUX_PROGRAM_FULL"
  EXTRA_BENCH_ARGS=(--full)
else
  FLUX_PROGRAM="$FLUX_PROGRAM_SMOKE"
  EXTRA_BENCH_ARGS=()
fi

FLUX_CMD="./target/release/flux $FLUX_PROGRAM"
PROFILE_CMD=("$PROFILE_PROGRAM")
PROFILE_LABEL="vm"
if [[ "$PROFILE_JIT" -eq 1 ]]; then
  PROFILE_CMD=("$PROFILE_PROGRAM" "--jit")
  PROFILE_LABEL="jit"
fi
FLAMEGRAPH_ARTIFACT="flamegraph-${BENCHMARK}-${PROFILE_LABEL}.svg"
TRACE_ARTIFACT="cargo-flamegraph-${BENCHMARK}-${PROFILE_LABEL}.trace"

echo "== Flux-only stability benchmark =="
hyperfine -N --warmup "$FLUX_WARMUP" --runs "$FLUX_RUNS" \
  --command-name "${BENCHMARK}/flux-only" \
  "$FLUX_CMD"
echo

echo "== Cross-language benchmark =="
BENCH_CMD=("$SCRIPT_DIR/bench.sh" "$BENCHMARK" --no-shell --runs "$RUNS" --warmup "$WARMUP")
if [[ "${#EXTRA_BENCH_ARGS[@]}" -gt 0 ]]; then
  BENCH_CMD+=("${EXTRA_BENCH_ARGS[@]}")
fi
"${BENCH_CMD[@]}"
echo

if [[ "$SKIP_FLAMEGRAPH" -eq 0 ]]; then
  echo "== Flamegraph generation =="
  echo "Profiling workload (${PROFILE_LABEL}): ${PROFILE_CMD[*]}"
  CARGO_PROFILE_RELEASE_DEBUG=true cargo flamegraph --reverse --bin flux -- "${PROFILE_CMD[@]}"
  if [[ -f flamegraph.svg ]]; then
    cp flamegraph.svg "$FLAMEGRAPH_ARTIFACT"
  fi
  if [[ -f cargo-flamegraph.trace ]]; then
    cp cargo-flamegraph.trace "$TRACE_ARTIFACT"
  fi
  echo
fi

if [[ ! -f "$FLAMEGRAPH_ARTIFACT" ]]; then
  echo "Error: ${FLAMEGRAPH_ARTIFACT} not found. Run flamegraph first." >&2
  exit 1
fi

echo "== Flamewatch (top ${TOP_N}) =="
perl -ne 'while(/<title>([^<]+)<\/title>/g){$t=$1; if($t =~ /^(.*) \(([0-9,]+) samples, ([0-9.]+)%\)$/){$n=$1;$p=$3; if(!defined $m{$n} || $p>$m{$n}){$m{$n}=$p;} }} END { for $k (keys %m){ printf "%.2f\t%s\n", $m{$k}, $k; } }' "$FLAMEGRAPH_ARTIFACT" \
  | sort -nr \
  | head -n "$TOP_N"
echo
echo "Saved flamegraph artifact: ${FLAMEGRAPH_ARTIFACT}"
