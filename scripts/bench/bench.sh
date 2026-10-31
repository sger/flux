#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

show_help() {
  cat <<'USAGE'
Usage:
  scripts/bench/bench.sh <benchmark> [options]
  scripts/bench/bench.sh <group> [options]

Benchmarks:
  binarytrees              Tree-building benchmark with smoke/full modes
  cfold                    Constant-folding benchmark
  deriv                    Symbolic differentiation benchmark
  nqueens                  N-Queens search benchmark
  qsort                    Quicksort array benchmark
  rbtree_ck                Red-black tree benchmark
  rbtree                   Red-black tree insert benchmark
  rbtree2                  Red-black tree big-int benchmark
  rbtree_del               Red-black tree delete benchmark
  nbody                    Planetary orbits, float arithmetic
  sfib                     Tail-recursive loop over wrapping 32-bit ints
  tak                      Takeuchi function, call overhead

Groups:
  core                     Curated suite for broad coverage
  extended                 Additional benchmark variants and stressors
  all                      Run core and extended benchmarks

Options:
  --runs <n>               Number of benchmark runs (default: 20)
  --warmup <n>             Warmup runs (default: 2)
  --full                   Use the full workload when supported
  --report-file <path>     Write a Markdown report (default depends on benchmark)
  --no-readme              Do not sync the latest table into benchmarks/README.md
  --save-baseline          Copy this run's JSON results to benchmarks/baselines/
  --flux-cmd <cmd>         Flux VM command
  --flux-native-cmd <cmd>  Flux native command (default: a binary emitted before the run)
  --rust-cmd <cmd>         Rust command
  --python-cmd <cmd>       Python command
  --haskell-cmd <cmd>      Haskell command
  --ocaml-cmd <cmd>        OCaml command
  --erlang-cmd <cmd>       Erlang command (empty disables the case)
  --elixir-cmd <cmd>       Elixir command (empty disables the case)
  --go-cmd <cmd>           Go command (empty disables the case)
  --vms                    Also time interpreted cases: Haskell under GHCi, Go under yaegi
  --no-flux-native         Disable Flux native benchmark case
  --no-shell               Use raw commands (no shell expansion)
  -h, --help               Show this help

Examples:
  scripts/bench/bench.sh binarytrees
  scripts/bench/bench.sh core
  scripts/bench/bench.sh extended --runs 10 --warmup 1
  scripts/bench/bench.sh binarytrees --full --runs 3 --warmup 1
  scripts/bench/bench.sh cfold --runs 30 --warmup 5
  scripts/bench/bench.sh deriv --runs 30 --warmup 5
  scripts/bench/bench.sh nqueens --runs 30 --warmup 5
  scripts/bench/bench.sh qsort --runs 30 --warmup 5
  scripts/bench/bench.sh rbtree_ck --runs 30 --warmup 5
  scripts/bench/bench.sh rbtree --runs 30 --warmup 5
  scripts/bench/bench.sh rbtree2 --runs 30 --warmup 5
  scripts/bench/bench.sh rbtree_del --runs 30 --warmup 5
  scripts/bench/bench.sh nbody --runs 10 --warmup 1
USAGE
}

CORE_BENCHMARKS=(binarytrees cfold deriv nqueens qsort rbtree_del nbody tak)
EXTENDED_BENCHMARKS=(rbtree_ck rbtree rbtree2 sfib)

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
  for arg in "${PASSTHROUGH_ARGS[@]-}"; do
    case "$arg" in
      --report-file|--flux-cmd|--flux-native-cmd|--rust-cmd|--python-cmd|--haskell-cmd|--ocaml-cmd|--erlang-cmd|--elixir-cmd|--go-cmd)
        echo "Error: option '$arg' is only supported for single benchmarks, not benchmark groups." >&2
        exit 1
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

RUNS=20
WARMUP=2
SHELL_MODE=1
ENABLE_FLUX_NATIVE=1
UPDATE_README=1
README_PATH="benchmarks/README.md"
FULL_MODE=0
SAVE_BASELINE=0
BENCH_DIR="target/bench"
BASELINE_DIR="benchmarks/baselines"

TITLE=""
REPORT_TAG=""
REPORT_FILE=""
SUPPORTS_FULL=0

FLUX_PROGRAM_SMOKE=""
FLUX_PROGRAM_FULL=""
DEFAULT_RUST_CMD_SMOKE=""
DEFAULT_RUST_CMD_FULL=""
DEFAULT_HASKELL_CMD_SMOKE=""
DEFAULT_HASKELL_CMD_FULL=""
DEFAULT_OCAML_CMD_SMOKE=""
DEFAULT_OCAML_CMD_FULL=""
DEFAULT_PYTHON_CMD_SMOKE=""
DEFAULT_PYTHON_CMD_FULL=""

FLUX_PROGRAM=""
FLUX_CMD=""
DEFAULT_FLUX_NATIVE_CMD=""
FLUX_NATIVE_CMD=""
RUST_CMD=""
PYTHON_CMD=""
HASKELL_CMD=""
OCAML_CMD=""
WORKLOAD_ARG_SMOKE=""
WORKLOAD_ARG_FULL=""
ERLANG_CMD=""
ELIXIR_CMD=""
GO_CMD=""
DEFAULT_ERLANG_CMD=""
DEFAULT_ELIXIR_CMD=""
DEFAULT_GO_CMD=""
GHCI_CMD=""
YAEGI_CMD=""
ENABLE_VMS=0

HAS_RUST=0
HAS_PYTHON=0
HAS_HASKELL=0
HAS_OCAML=0

configure_benchmark() {
  case "$BENCHMARK" in
    binarytrees)
      TITLE="Binary Trees"
      REPORT_TAG="binarytrees"
      REPORT_FILE="reports/BINARYTREES_REPORT.md"
      SUPPORTS_FULL=1
      FLUX_PROGRAM_SMOKE="benchmarks/flux/binarytrees_smoke.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/binarytrees.flx"
      DEFAULT_RUST_CMD_SMOKE="./target/release/binarytrees_rust 8"
      DEFAULT_RUST_CMD_FULL="./target/release/binarytrees_rust 21"
      DEFAULT_HASKELL_CMD_SMOKE="./target/release/binarytrees_hs 8"
      DEFAULT_HASKELL_CMD_FULL="./target/release/binarytrees_hs 21"
      DEFAULT_OCAML_CMD_SMOKE="./target/release/binarytrees_ocaml 8"
      DEFAULT_OCAML_CMD_FULL="./target/release/binarytrees_ocaml 21"
      DEFAULT_PYTHON_CMD_SMOKE="python3 benchmarks/python/binarytrees.py 8"
      DEFAULT_PYTHON_CMD_FULL="python3 benchmarks/python/binarytrees.py 21"
      WORKLOAD_ARG_SMOKE="8"
      WORKLOAD_ARG_FULL="21"
      HAS_RUST=1
      HAS_PYTHON=1
      HAS_HASKELL=1
      HAS_OCAML=1
      ;;
    cfold)
      TITLE="Constant Folding"
      REPORT_TAG="cfold"
      REPORT_FILE="reports/CFOLD_REPORT.md"
      FLUX_PROGRAM_SMOKE="benchmarks/flux/cfold.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/cfold.flx"
      DEFAULT_RUST_CMD_SMOKE="./target/release/cfold_rust"
      DEFAULT_RUST_CMD_FULL="./target/release/cfold_rust"
      DEFAULT_HASKELL_CMD_SMOKE="./target/release/cfold_hs"
      DEFAULT_HASKELL_CMD_FULL="./target/release/cfold_hs"
      DEFAULT_OCAML_CMD_SMOKE="./target/release/cfold_ocaml"
      DEFAULT_OCAML_CMD_FULL="./target/release/cfold_ocaml"
      DEFAULT_PYTHON_CMD_SMOKE="python3 benchmarks/python/cfold.py"
      DEFAULT_PYTHON_CMD_FULL="python3 benchmarks/python/cfold.py"
      HAS_RUST=1
      HAS_PYTHON=1
      HAS_HASKELL=1
      HAS_OCAML=1
      ;;
    deriv)
      TITLE="Symbolic Differentiation"
      REPORT_TAG="deriv"
      REPORT_FILE="reports/DERIV_REPORT.md"
      FLUX_PROGRAM_SMOKE="benchmarks/flux/deriv.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/deriv.flx"
      DEFAULT_RUST_CMD_SMOKE="./target/release/deriv_rust"
      DEFAULT_RUST_CMD_FULL="./target/release/deriv_rust"
      DEFAULT_HASKELL_CMD_SMOKE="./target/release/deriv_hs"
      DEFAULT_HASKELL_CMD_FULL="./target/release/deriv_hs"
      DEFAULT_OCAML_CMD_SMOKE="./target/release/deriv_ocaml"
      DEFAULT_OCAML_CMD_FULL="./target/release/deriv_ocaml"
      DEFAULT_PYTHON_CMD_SMOKE="python3 benchmarks/python/deriv.py"
      DEFAULT_PYTHON_CMD_FULL="python3 benchmarks/python/deriv.py"
      HAS_RUST=1
      HAS_PYTHON=1
      HAS_HASKELL=1
      HAS_OCAML=1
      ;;
    nqueens)
      TITLE="N-Queens"
      REPORT_TAG="nqueens"
      REPORT_FILE="reports/NQUEENS_REPORT.md"
      FLUX_PROGRAM_SMOKE="benchmarks/flux/nqueens.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/nqueens.flx"
      DEFAULT_RUST_CMD_SMOKE="./target/release/nqueens_rust"
      DEFAULT_RUST_CMD_FULL="./target/release/nqueens_rust"
      DEFAULT_HASKELL_CMD_SMOKE="./target/release/nqueens_hs"
      DEFAULT_HASKELL_CMD_FULL="./target/release/nqueens_hs"
      DEFAULT_OCAML_CMD_SMOKE="./target/release/nqueens_ocaml"
      DEFAULT_OCAML_CMD_FULL="./target/release/nqueens_ocaml"
      DEFAULT_PYTHON_CMD_SMOKE="python3 benchmarks/python/nqueens.py"
      DEFAULT_PYTHON_CMD_FULL="python3 benchmarks/python/nqueens.py"
      HAS_RUST=1
      HAS_PYTHON=1
      HAS_HASKELL=1
      HAS_OCAML=1
      ;;
    qsort)
      TITLE="Quicksort"
      REPORT_TAG="qsort"
      REPORT_FILE="reports/QSORT_REPORT.md"
      FLUX_PROGRAM_SMOKE="benchmarks/flux/qsort.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/qsort.flx"
      DEFAULT_RUST_CMD_SMOKE="./target/release/qsort_rust"
      DEFAULT_RUST_CMD_FULL="./target/release/qsort_rust"
      DEFAULT_HASKELL_CMD_SMOKE="./target/release/qsort_hs"
      DEFAULT_HASKELL_CMD_FULL="./target/release/qsort_hs"
      DEFAULT_OCAML_CMD_SMOKE="./target/release/qsort_ocaml"
      DEFAULT_OCAML_CMD_FULL="./target/release/qsort_ocaml"
      DEFAULT_PYTHON_CMD_SMOKE="python3 benchmarks/python/qsort.py"
      DEFAULT_PYTHON_CMD_FULL="python3 benchmarks/python/qsort.py"
      HAS_RUST=1
      HAS_PYTHON=1
      HAS_HASKELL=1
      HAS_OCAML=1
      ;;
    rbtree_ck)
      TITLE="Red-Black Tree"
      REPORT_TAG="rbtree-ck"
      REPORT_FILE="reports/RBTREE_CK_REPORT.md"
      FLUX_PROGRAM_SMOKE="benchmarks/flux/rbtree_ck.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/rbtree_ck.flx"
      DEFAULT_RUST_CMD_SMOKE="./target/release/rbtree_ck_rust"
      DEFAULT_RUST_CMD_FULL="./target/release/rbtree_ck_rust"
      DEFAULT_HASKELL_CMD_SMOKE="./target/release/rbtree_ck_hs"
      DEFAULT_HASKELL_CMD_FULL="./target/release/rbtree_ck_hs"
      DEFAULT_OCAML_CMD_SMOKE="./target/release/rbtree_ck_ocaml"
      DEFAULT_OCAML_CMD_FULL="./target/release/rbtree_ck_ocaml"
      DEFAULT_PYTHON_CMD_SMOKE="python3 benchmarks/python/rbtree_ck.py"
      DEFAULT_PYTHON_CMD_FULL="python3 benchmarks/python/rbtree_ck.py"
      HAS_RUST=1
      HAS_PYTHON=1
      HAS_HASKELL=1
      HAS_OCAML=1
      ;;
    rbtree)
      TITLE="Red-Black Tree Insert"
      REPORT_TAG="rbtree"
      REPORT_FILE="reports/RBTREE_REPORT.md"
      FLUX_PROGRAM_SMOKE="benchmarks/flux/rbtree.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/rbtree.flx"
      DEFAULT_RUST_CMD_SMOKE="./target/release/rbtree_rust"
      DEFAULT_RUST_CMD_FULL="./target/release/rbtree_rust"
      DEFAULT_HASKELL_CMD_SMOKE="./target/release/rbtree_hs"
      DEFAULT_HASKELL_CMD_FULL="./target/release/rbtree_hs"
      DEFAULT_OCAML_CMD_SMOKE="./target/release/rbtree_ocaml"
      DEFAULT_OCAML_CMD_FULL="./target/release/rbtree_ocaml"
      DEFAULT_PYTHON_CMD_SMOKE="python3 benchmarks/python/rbtree.py"
      DEFAULT_PYTHON_CMD_FULL="python3 benchmarks/python/rbtree.py"
      HAS_RUST=1
      HAS_PYTHON=1
      HAS_HASKELL=1
      HAS_OCAML=1
      ;;
    rbtree2)
      TITLE="Red-Black Tree Insert BigInt"
      REPORT_TAG="rbtree2"
      REPORT_FILE="reports/RBTREE2_REPORT.md"
      FLUX_PROGRAM_SMOKE="benchmarks/flux/rbtree2.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/rbtree2.flx"
      DEFAULT_RUST_CMD_SMOKE="./target/release/rbtree2_rust"
      DEFAULT_RUST_CMD_FULL="./target/release/rbtree2_rust"
      DEFAULT_HASKELL_CMD_SMOKE="./target/release/rbtree2_hs"
      DEFAULT_HASKELL_CMD_FULL="./target/release/rbtree2_hs"
      DEFAULT_OCAML_CMD_SMOKE="./target/release/rbtree2_ocaml"
      DEFAULT_OCAML_CMD_FULL="./target/release/rbtree2_ocaml"
      DEFAULT_PYTHON_CMD_SMOKE="python3 benchmarks/python/rbtree2.py"
      DEFAULT_PYTHON_CMD_FULL="python3 benchmarks/python/rbtree2.py"
      HAS_RUST=1
      HAS_PYTHON=1
      HAS_HASKELL=1
      HAS_OCAML=1
      ;;
    rbtree_del)
      TITLE="Red-Black Tree Delete"
      REPORT_TAG="rbtree-del"
      REPORT_FILE="reports/RBTREE_DEL_REPORT.md"
      FLUX_PROGRAM_SMOKE="benchmarks/flux/rbtree_del.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/rbtree_del.flx"
      DEFAULT_RUST_CMD_SMOKE="./target/release/rbtree_del_rust"
      DEFAULT_RUST_CMD_FULL="./target/release/rbtree_del_rust"
      DEFAULT_HASKELL_CMD_SMOKE="./target/release/rbtree_del_hs"
      DEFAULT_HASKELL_CMD_FULL="./target/release/rbtree_del_hs"
      DEFAULT_OCAML_CMD_SMOKE="./target/release/rbtree_del_ocaml"
      DEFAULT_OCAML_CMD_FULL="./target/release/rbtree_del_ocaml"
      DEFAULT_PYTHON_CMD_SMOKE="python3 benchmarks/python/rbtree_del.py"
      DEFAULT_PYTHON_CMD_FULL="python3 benchmarks/python/rbtree_del.py"
      HAS_RUST=1
      HAS_PYTHON=1
      HAS_HASKELL=1
      HAS_OCAML=1
      ;;
    nbody)
      TITLE="N-Body"
      REPORT_TAG="nbody"
      REPORT_FILE="reports/NBODY_REPORT.md"
      FLUX_PROGRAM_SMOKE="benchmarks/flux/nbody.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/nbody.flx"
      DEFAULT_RUST_CMD_SMOKE="./target/release/nbody_rust"
      DEFAULT_RUST_CMD_FULL="./target/release/nbody_rust"
      DEFAULT_HASKELL_CMD_SMOKE="./target/release/nbody_hs"
      DEFAULT_HASKELL_CMD_FULL="./target/release/nbody_hs"
      DEFAULT_OCAML_CMD_SMOKE="./target/release/nbody_ocaml"
      DEFAULT_OCAML_CMD_FULL="./target/release/nbody_ocaml"
      DEFAULT_PYTHON_CMD_SMOKE="python3 benchmarks/python/nbody.py"
      DEFAULT_PYTHON_CMD_FULL="python3 benchmarks/python/nbody.py"
      HAS_RUST=1
      HAS_PYTHON=1
      HAS_HASKELL=1
      HAS_OCAML=1
      ;;
    sfib)
      TITLE="Wrapping Fibonacci"
      REPORT_TAG="sfib"
      REPORT_FILE="reports/SFIB_REPORT.md"
      FLUX_PROGRAM_SMOKE="benchmarks/flux/sfib.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/sfib.flx"
      DEFAULT_RUST_CMD_SMOKE="./target/release/sfib_rust"
      DEFAULT_RUST_CMD_FULL="./target/release/sfib_rust"
      DEFAULT_HASKELL_CMD_SMOKE="./target/release/sfib_hs"
      DEFAULT_HASKELL_CMD_FULL="./target/release/sfib_hs"
      DEFAULT_OCAML_CMD_SMOKE="./target/release/sfib_ocaml"
      DEFAULT_OCAML_CMD_FULL="./target/release/sfib_ocaml"
      DEFAULT_PYTHON_CMD_SMOKE="python3 benchmarks/python/sfib.py"
      DEFAULT_PYTHON_CMD_FULL="python3 benchmarks/python/sfib.py"
      HAS_RUST=1
      HAS_PYTHON=1
      HAS_HASKELL=1
      HAS_OCAML=1
      ;;
    tak)
      TITLE="Takeuchi"
      REPORT_TAG="tak"
      REPORT_FILE="reports/TAK_REPORT.md"
      FLUX_PROGRAM_SMOKE="benchmarks/flux/tak.flx"
      FLUX_PROGRAM_FULL="benchmarks/flux/tak.flx"
      DEFAULT_RUST_CMD_SMOKE="./target/release/tak_rust"
      DEFAULT_RUST_CMD_FULL="./target/release/tak_rust"
      DEFAULT_HASKELL_CMD_SMOKE="./target/release/tak_hs"
      DEFAULT_HASKELL_CMD_FULL="./target/release/tak_hs"
      DEFAULT_OCAML_CMD_SMOKE="./target/release/tak_ocaml"
      DEFAULT_OCAML_CMD_FULL="./target/release/tak_ocaml"
      DEFAULT_PYTHON_CMD_SMOKE="python3 benchmarks/python/tak.py"
      DEFAULT_PYTHON_CMD_FULL="python3 benchmarks/python/tak.py"
      HAS_RUST=1
      HAS_PYTHON=1
      HAS_HASKELL=1
      HAS_OCAML=1
      ;;
    *)
      echo "Error: unknown benchmark '$BENCHMARK'." >&2
      show_help >&2
      exit 1
      ;;
  esac

  FLUX_PROGRAM="$FLUX_PROGRAM_SMOKE"
  FLUX_CMD="./target/release/flux $FLUX_PROGRAM"
  DEFAULT_FLUX_NATIVE_CMD="./${BENCH_DIR}/${BENCHMARK}_flux"
  FLUX_NATIVE_CMD="$DEFAULT_FLUX_NATIVE_CMD"
  RUST_CMD="$DEFAULT_RUST_CMD_SMOKE"
  PYTHON_CMD="$DEFAULT_PYTHON_CMD_SMOKE"
  HASKELL_CMD="$DEFAULT_HASKELL_CMD_SMOKE"
  OCAML_CMD="$DEFAULT_OCAML_CMD_SMOKE"
  configure_port_cmds "$WORKLOAD_ARG_SMOKE"
}

# The Erlang, Elixir and Go ports follow one naming scheme, so their commands
# come from the benchmark name instead of a per-benchmark table. A command the
# user overrode (or emptied to disable) is kept when `--full` reconfigures.
configure_port_cmds() {
  local arg="$1"
  local module="" part erlang elixir go
  local -a parts
  IFS=_ read -ra parts <<< "$BENCHMARK"
  for part in "${parts[@]}"; do
    module+="${part^}"
  done
  erlang="erl -noshell -pa ${BENCH_DIR}/erlang -run ${BENCHMARK} main${arg:+ $arg} -s init stop"
  elixir="elixir -pa ${BENCH_DIR}/elixir -e 'Bench.${module}.main(System.argv())'${arg:+ -- $arg}"
  go="./target/release/${BENCHMARK}_go${arg:+ $arg}"
  [[ "$ERLANG_CMD" == "$DEFAULT_ERLANG_CMD" ]] && ERLANG_CMD="$erlang"
  [[ "$ELIXIR_CMD" == "$DEFAULT_ELIXIR_CMD" ]] && ELIXIR_CMD="$elixir"
  [[ "$GO_CMD" == "$DEFAULT_GO_CMD" ]] && GO_CMD="$go"
  DEFAULT_ERLANG_CMD="$erlang"
  DEFAULT_ELIXIR_CMD="$elixir"
  DEFAULT_GO_CMD="$go"
  GHCI_CMD="ghc -e ':main${arg:+ $arg}' benchmarks/haskell/${BENCHMARK//_/-}.hs"
  YAEGI_CMD="yaegi run benchmarks/go/${BENCHMARK}/main.go${arg:+ $arg}"
}

configure_benchmark

while [[ $# -gt 0 ]]; do
  case "$1" in
    --runs)
      RUNS="$2"
      shift 2
      ;;
    --warmup)
      WARMUP="$2"
      shift 2
      ;;
    --full)
      FULL_MODE=1
      shift
      ;;
    --report-file)
      REPORT_FILE="$2"
      shift 2
      ;;
    --no-readme)
      UPDATE_README=0
      shift
      ;;
    --save-baseline)
      SAVE_BASELINE=1
      shift
      ;;
    --flux-cmd)
      FLUX_CMD="$2"
      shift 2
      ;;
    --flux-native-cmd)
      FLUX_NATIVE_CMD="$2"
      shift 2
      ;;
    --rust-cmd)
      RUST_CMD="$2"
      shift 2
      ;;
    --python-cmd)
      PYTHON_CMD="$2"
      shift 2
      ;;
    --haskell-cmd)
      HASKELL_CMD="$2"
      shift 2
      ;;
    --ocaml-cmd)
      OCAML_CMD="$2"
      shift 2
      ;;
    --erlang-cmd)
      ERLANG_CMD="$2"
      shift 2
      ;;
    --elixir-cmd)
      ELIXIR_CMD="$2"
      shift 2
      ;;
    --go-cmd)
      GO_CMD="$2"
      shift 2
      ;;
    --vms)
      ENABLE_VMS=1
      shift
      ;;
    --no-flux-native)
      ENABLE_FLUX_NATIVE=0
      shift
      ;;
    --no-shell)
      SHELL_MODE=0
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

if [[ "$FULL_MODE" -eq 1 ]]; then
  if [[ "$SUPPORTS_FULL" -ne 1 ]]; then
    echo "Error: benchmark '$BENCHMARK' does not define a separate full workload." >&2
    exit 1
  fi
  FLUX_PROGRAM="$FLUX_PROGRAM_FULL"
  FLUX_CMD="./target/release/flux $FLUX_PROGRAM"
  RUST_CMD="$DEFAULT_RUST_CMD_FULL"
  PYTHON_CMD="$DEFAULT_PYTHON_CMD_FULL"
  HASKELL_CMD="$DEFAULT_HASKELL_CMD_FULL"
  OCAML_CMD="$DEFAULT_OCAML_CMD_FULL"
  configure_port_cmds "$WORKLOAD_ARG_FULL"
fi

if [[ ! -x "./target/release/flux" ]]; then
  echo "Error: missing ./target/release/flux" >&2
  echo "Build it with: cargo build --release --features llvm --bin flux" >&2
  exit 1
fi

if ! command -v hyperfine >/dev/null 2>&1; then
  echo "Error: hyperfine not found." >&2
  echo "Install: brew install hyperfine" >&2
  exit 1
fi

if [[ "$HAS_RUST" -eq 1 ]] && [[ "$RUST_CMD" == "$DEFAULT_RUST_CMD_SMOKE" || "$RUST_CMD" == "$DEFAULT_RUST_CMD_FULL" ]]; then
  rust_bin="${DEFAULT_RUST_CMD_SMOKE%% *}"
  if [[ ! -x "$rust_bin" ]]; then
    echo "Error: missing $rust_bin" >&2
    echo "Build it with: cargo build --release --bin ${BENCHMARK}_rust" >&2
    exit 1
  fi
fi

if [[ "$HAS_HASKELL" -eq 1 ]] && [[ "$HASKELL_CMD" == "$DEFAULT_HASKELL_CMD_SMOKE" || "$HASKELL_CMD" == "$DEFAULT_HASKELL_CMD_FULL" ]]; then
  haskell_bin="${DEFAULT_HASKELL_CMD_SMOKE%% *}"
  if [[ ! -x "$haskell_bin" ]]; then
    echo "Error: missing $haskell_bin" >&2
    echo "Build it with: ghc -O2 benchmarks/haskell/${BENCHMARK}.hs -o ${haskell_bin}" >&2
    exit 1
  fi
fi

if [[ "$HAS_OCAML" -eq 1 ]] && [[ "$OCAML_CMD" == "$DEFAULT_OCAML_CMD_SMOKE" || "$OCAML_CMD" == "$DEFAULT_OCAML_CMD_FULL" ]]; then
  ocaml_bin="${DEFAULT_OCAML_CMD_SMOKE%% *}"
  if [[ ! -x "$ocaml_bin" ]]; then
    if command -v ocamlopt >/dev/null 2>&1; then
      echo "Building $ocaml_bin with ocamlopt..." >&2
      if [[ "$BENCHMARK" == "binarytrees" ]]; then
        ocamlopt -I +unix unix.cmxa -o "$ocaml_bin" "benchmarks/ocaml/${BENCHMARK}.ml"
      else
        ocamlopt -o "$ocaml_bin" "benchmarks/ocaml/${BENCHMARK}.ml"
      fi
      if [[ ! -x "$ocaml_bin" ]]; then
        echo "Error: failed to build $ocaml_bin with ocamlopt." >&2
        exit 1
      fi
    fi
    if [[ ! -x "$ocaml_bin" ]]; then
      echo "Warning: skipping OCaml benchmark case because $ocaml_bin is missing and ocamlopt is not installed." >&2
      HAS_OCAML=0
      OCAML_CMD=""
    fi
  fi
fi

mkdir -p "$BENCH_DIR" "$(dirname "$REPORT_FILE")"

# Each port is built only when its default command is in use; a missing
# toolchain drops that case with a warning, as for OCaml above.
if [[ -n "$ERLANG_CMD" && "$ERLANG_CMD" == "$DEFAULT_ERLANG_CMD" ]]; then
  if command -v erlc >/dev/null 2>&1; then
    mkdir -p "${BENCH_DIR}/erlang"
    erlc -o "${BENCH_DIR}/erlang" "benchmarks/erlang/${BENCHMARK}.erl"
  else
    echo "Warning: skipping the Erlang case because erlc is not installed." >&2
    ERLANG_CMD=""
  fi
fi

if [[ -n "$ELIXIR_CMD" && "$ELIXIR_CMD" == "$DEFAULT_ELIXIR_CMD" ]]; then
  if command -v elixirc >/dev/null 2>&1; then
    mkdir -p "${BENCH_DIR}/elixir"
    elixirc -o "${BENCH_DIR}/elixir" "benchmarks/elixir/${BENCHMARK}.ex"
  else
    echo "Warning: skipping the Elixir case because elixirc is not installed." >&2
    ELIXIR_CMD=""
  fi
fi

if [[ -n "$GO_CMD" && "$GO_CMD" == "$DEFAULT_GO_CMD" ]]; then
  if command -v go >/dev/null 2>&1; then
    (cd benchmarks/go && go build -o "${REPO_ROOT}/target/release/${BENCHMARK}_go" "./${BENCHMARK}")
  else
    echo "Warning: skipping the Go case because go is not installed." >&2
    GO_CMD=""
  fi
fi

if [[ "$ENABLE_VMS" -eq 1 ]]; then
  if ! command -v ghc >/dev/null 2>&1; then
    echo "Warning: skipping the GHCi case because ghc is not installed." >&2
    GHCI_CMD=""
  fi
  if ! command -v yaegi >/dev/null 2>&1; then
    echo "Warning: skipping the yaegi case because yaegi is not on PATH." >&2
    echo "Install: go install github.com/traefik/yaegi/cmd/yaegi@latest" >&2
    YAEGI_CMD=""
  fi
fi

if [[ "$ENABLE_FLUX_NATIVE" -eq 1 && "$FLUX_NATIVE_CMD" == "$DEFAULT_FLUX_NATIVE_CMD" ]]; then
  echo "Building ${FLUX_NATIVE_CMD} from ${FLUX_PROGRAM}..." >&2
  if ! ./target/release/flux "$FLUX_PROGRAM" --native --emit-binary -o "${BENCH_DIR}/${BENCHMARK}_flux" >&2; then
    echo "Error: failed to emit a native binary for $FLUX_PROGRAM." >&2
    echo "Make sure flux was built with: cargo build --release --features llvm --bin flux" >&2
    exit 1
  fi
fi

names=()
commands=()

add_case() {
  local name="$1"
  local cmd="$2"
  if [[ -n "$cmd" ]]; then
    names+=("$name")
    commands+=("$cmd")
  fi
}

add_case "${BENCHMARK}/flux" "$FLUX_CMD"
if [[ "$ENABLE_FLUX_NATIVE" -eq 1 ]]; then
  add_case "${BENCHMARK}/flux-native" "$FLUX_NATIVE_CMD"
fi
if [[ "$HAS_RUST" -eq 1 ]]; then
  add_case "${BENCHMARK}/rust" "$RUST_CMD"
fi
if [[ "$HAS_PYTHON" -eq 1 ]]; then
  add_case "${BENCHMARK}/python" "$PYTHON_CMD"
fi
if [[ "$HAS_HASKELL" -eq 1 ]]; then
  add_case "${BENCHMARK}/haskell" "$HASKELL_CMD"
fi
if [[ "$HAS_OCAML" -eq 1 ]]; then
  add_case "${BENCHMARK}/ocaml" "$OCAML_CMD"
fi
add_case "${BENCHMARK}/erlang" "$ERLANG_CMD"
add_case "${BENCHMARK}/elixir" "$ELIXIR_CMD"
add_case "${BENCHMARK}/go" "$GO_CMD"
if [[ "$ENABLE_VMS" -eq 1 ]]; then
  add_case "${BENCHMARK}/haskell-ghci" "$GHCI_CMD"
  add_case "${BENCHMARK}/go-yaegi" "$YAEGI_CMD"
fi

echo "Benchmark: $BENCHMARK"
echo "Runs: $RUNS, Warmup: $WARMUP"
for i in "${!names[@]}"; do
  echo "  - ${names[$i]} => ${commands[$i]}"
done
echo

results_suffix=""
if [[ "$FULL_MODE" -eq 1 ]]; then
  results_suffix="_full"
fi
RESULTS_JSON="${BENCH_DIR}/${BENCHMARK}${results_suffix}.json"
BASELINE_JSON="${BASELINE_DIR}/${BENCHMARK}${results_suffix}.json"

hf_cmd=(hyperfine --warmup "$WARMUP" --runs "$RUNS")
if [[ "$SHELL_MODE" -eq 0 ]]; then
  hf_cmd+=(--shell none)
fi

for name in "${names[@]}"; do
  hf_cmd+=(--command-name "$name")
done
for command in "${commands[@]}"; do
  hf_cmd+=("$command")
done
hf_cmd+=(--export-markdown "$REPORT_FILE" --export-json "$RESULTS_JSON")

"${hf_cmd[@]}"

if [[ "$SAVE_BASELINE" -eq 1 ]]; then
  mkdir -p "$BASELINE_DIR"
  cp "$RESULTS_JSON" "$BASELINE_JSON"
  echo "Saved baseline: $BASELINE_JSON"
fi

# The Windows Store's python3 alias exists on PATH but only prints an install hint.
python_bin=""
for candidate in python3 python; do
  if "$candidate" -c '' >/dev/null 2>&1; then
    python_bin="$candidate"
    break
  fi
done
comparison=""
if [[ -n "$python_bin" ]]; then
  comparison="$("$python_bin" "$SCRIPT_DIR/compare.py" "$RESULTS_JSON" "$BASELINE_JSON" \
    --vm "${BENCHMARK}/flux" --native "${BENCHMARK}/flux-native")"
else
  echo "Warning: no Python interpreter found; skipping the baseline comparison." >&2
fi

timestamp="$(date -u +"%Y-%m-%d %H:%M:%S UTC")"
tmp_report="$(mktemp "${TMPDIR:-/tmp}/${REPORT_TAG}_report.XXXXXX.md")"
{
  echo "# ${TITLE} Benchmark Report"
  echo
  echo "- Generated: $timestamp"
  echo "- Runs: $RUNS"
  echo "- Warmup: $WARMUP"
  if [[ "$SUPPORTS_FULL" -eq 1 ]]; then
    echo "- Full baseline: $(if [[ "$FULL_MODE" -eq 1 ]]; then echo yes; else echo no; fi)"
  fi
  echo
  cat "$REPORT_FILE"
  if [[ -n "$comparison" ]]; then
    echo
    echo "$comparison"
  fi
} > "$tmp_report"
mv "$tmp_report" "$REPORT_FILE"
if [[ -n "$comparison" ]]; then
  echo
  echo "$comparison"
fi
echo
echo "Wrote report: $REPORT_FILE"
echo "Wrote results: $RESULTS_JSON"

if [[ "$UPDATE_README" -eq 1 ]]; then
  marker_start="<!-- ${REPORT_TAG}-report:start -->"
  marker_end="<!-- ${REPORT_TAG}-report:end -->"
  if grep -Fq "$marker_start" "$README_PATH" && grep -Fq "$marker_end" "$README_PATH"; then
    tmp_readme="$(mktemp "${TMPDIR:-/tmp}/${REPORT_TAG}_readme.XXXXXX.md")"
    awk -v report="$REPORT_FILE" -v marker_start="$marker_start" -v marker_end="$marker_end" '
      BEGIN {
        in_block = 0
      }
      index($0, marker_start) {
        print
        while ((getline line < report) > 0) {
          print line
        }
        close(report)
        in_block = 1
        next
      }
      index($0, marker_end) {
        in_block = 0
        print
        next
      }
      !in_block {
        print
      }
    ' "$README_PATH" > "$tmp_readme"
    mv "$tmp_readme" "$README_PATH"
    echo "Updated README: $README_PATH"
  else
    echo "Skipped README update: missing ${REPORT_TAG} report markers in $README_PATH"
  fi
fi
