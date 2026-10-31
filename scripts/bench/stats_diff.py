#!/usr/bin/env python3
"""Compare two `flux --stats-json` files phase by phase, as Markdown.

Usage: stats_diff.py <base.json> <head.json>

Phases are matched by name, so a VM file and a native file share only the
phases both have. Informational only: always exits 0 once both files load.
"""

import json
import sys
from pathlib import Path

COUNTS = ("source_lines", "globals", "functions", "instruction_bytes")


def load(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def fmt(ms):
    if ms is None:
        return "-"
    if ms < 1000.0:
        return f"{ms:.1f} ms"
    return f"{ms / 1000:.3f} s"


def ratio(base, head):
    if base is None or head is None or base == 0:
        return "-"
    return f"{head / base:.2f}x"


def counts(stats):
    found = {key: stats.get(key) for key in COUNTS}
    for key, value in (stats.get("modules") or {}).items():
        found[f"modules.{key}"] = value
    return found


def main(argv):
    if len(argv) != 2:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    base_path, head_path = Path(argv[0]), Path(argv[1])
    base, head = load(base_path), load(head_path)

    print(f"Base: `{base_path.as_posix()}` ({base.get('backend')})")
    print(f"Head: `{head_path.as_posix()}` ({head.get('backend')})")
    print()
    print("| Phase | Base | Head | Head / base |")
    print("|:---|---:|---:|---:|")
    base_ms = {p["phase"]: p["ms"] for p in base.get("phases", [])}
    head_ms = {p["phase"]: p["ms"] for p in head.get("phases", [])}
    # Head's order first, then any phase only base has, so a renamed or
    # dropped phase still shows up.
    names = list(head_ms) + [name for name in base_ms if name not in head_ms]
    for name in names:
        b, h = base_ms.get(name), head_ms.get(name)
        print(f"| `{name}` | {fmt(b)} | {fmt(h)} | {ratio(b, h)} |")
    total_b, total_h = sum(base_ms.values()), sum(head_ms.values())
    print(f"| total | {fmt(total_b)} | {fmt(total_h)} | {ratio(total_b, total_h)} |")

    base_counts, head_counts = counts(base), counts(head)
    changed = [key for key in head_counts if base_counts.get(key) != head_counts[key]]
    if changed:
        print()
        print("Counts that changed:")
        print()
        for key in changed:
            before, after = json.dumps(base_counts.get(key)), json.dumps(head_counts[key])
            print(f"- `{key}`: {before} -> {after}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
