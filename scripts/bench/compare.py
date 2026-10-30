#!/usr/bin/env python3
"""Compare a hyperfine JSON export with a baseline export, as Markdown.

Usage: compare.py <results.json> <baseline.json> [--vm <name> --native <name>]

Informational only: always exits 0.
"""

import json
import sys
from pathlib import Path


def load_means(path):
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    means = {}
    for r in data["results"]:
        # hyperfine 2 moved the timings under `summary` and the command name to
        # `name`; a baseline may come from either version.
        if "summary" in r:
            means[r["name"]] = r["summary"]["time_wall_clock"]["mean"]
        else:
            means[r["command"]] = r["mean"]
    return means


def fmt(seconds):
    if seconds < 1.0:
        return f"{seconds * 1000:.1f} ms"
    return f"{seconds:.3f} s"


def main(argv):
    args = list(argv)
    vm_name = native_name = None
    if "--vm" in args:
        i = args.index("--vm")
        vm_name = args[i + 1]
        del args[i : i + 2]
    if "--native" in args:
        i = args.index("--native")
        native_name = args[i + 1]
        del args[i : i + 2]
    if len(args) != 2:
        print(__doc__.strip(), file=sys.stderr)
        return 0

    results_path, baseline_path = Path(args[0]), Path(args[1])
    now = load_means(results_path)

    print("### Against baseline")
    print()
    if baseline_path.is_file():
        base = load_means(baseline_path)
        print(f"Baseline: `{baseline_path.as_posix()}`")
        print()
        print("| Command | Mean now | Baseline mean | Now / baseline |")
        print("|:---|---:|---:|---:|")
        for name, mean in now.items():
            if name in base:
                ratio = f"{mean / base[name]:.2f}x"
                base_mean = fmt(base[name])
            else:
                ratio = base_mean = "-"
            print(f"| `{name}` | {fmt(mean)} | {base_mean} | {ratio} |")
    else:
        print(
            f"No baseline at `{baseline_path.as_posix()}`; "
            "run with `--save-baseline` to record one."
        )

    if vm_name in now and native_name in now:
        print()
        print(f"VM / native: {now[vm_name] / now[native_name]:.2f}x")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
