#!/usr/bin/env python3

# Port of benchmarks/flux/nqueens_choice_direct.flx: every row is tried in
# a loop instead of by resuming `pick`.

from __future__ import annotations

import sys


def safe(queen: int, diag: int, xs) -> bool:
    while xs is not None:
        q, xs = xs
        if queen == q or queen == q + diag or queen == q - diag:
            return False
        diag += 1
    return True


def place(size: int, column: int, rest) -> int:
    if column > size:
        return 1
    a = 0
    for row in range(1, size + 1):
        if safe(row, 1, rest):
            a += place(size, column + 1, (row, rest))
    return a


def main() -> int:
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 12
    print(place(n, 1, None))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
