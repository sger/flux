#!/usr/bin/env python3

# Port of benchmarks/flux/product_early_direct.flx: the product returns 0
# at the first zero.

from __future__ import annotations

import sys


def product(xs) -> int:
    if xs is None:
        return 0
    y, ys = xs
    if y == 0:
        return 0
    return y * product(ys)


def enumerate_down(i: int):
    xs = None
    for v in range(i + 1):
        xs = (v, xs)
    return xs


def main() -> int:
    # The product recurses through all 1001 elements.
    sys.setrecursionlimit(5000)
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 100000
    xs = enumerate_down(1000)
    a = 0
    for _ in range(n):
        a += product(xs)
    print(a)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
