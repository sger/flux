#!/usr/bin/env python3

# Port of benchmarks/flux/generator_direct.flx: the tree is summed by a
# walk in order. The recursion is only as deep as the tree is high.

from __future__ import annotations

import sys


def make(n: int):
    if n == 0:
        return None
    t = make(n - 1)
    return (t, n, t)


def total(t, a: int) -> int:
    if t is None:
        return a
    l, v, r = t
    return total(r, total(l, a) + v)


def main() -> int:
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 25
    print(total(make(n), 0))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
