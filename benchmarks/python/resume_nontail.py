#!/usr/bin/env python3

# Port of benchmarks/flux/resume_nontail_direct.flx: the work after each
# resume is the work after a call.

from __future__ import annotations

import sys


def combine(x: int, y: int) -> int:
    return abs(x - 503 * y + 37) % 1009


def run(i: int, s: int) -> int:
    if i == 0:
        return s
    return combine(i, run(i - 1, s))


def main() -> int:
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 10000
    # The recursion is n deep.
    sys.setrecursionlimit(n + 1000)
    s = 0
    for _ in range(1000):
        s = run(n, s)
    print(s)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
