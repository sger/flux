#!/usr/bin/env python3

# Port of benchmarks/flux/triples_direct.flx: the triples i > j > k that sum
# to n are visited in nested loops and their hashes added as they are found.

from __future__ import annotations

import sys

M = 1000000007


def run(n: int) -> int:
    acc = 0
    for i in range(n, 0, -1):
        for j in range(i - 1, 0, -1):
            for k in range(j - 1, 0, -1):
                if i + j + k == n:
                    acc = (acc + (53 * i + 2809 * j + 148877 * k) % M) % M
    return acc


def main() -> int:
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 10
    print(run(n))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
