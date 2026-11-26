#!/usr/bin/env python3

# Port of benchmarks/flux/tree_explore_direct.flx: the state is threaded by
# hand through the branches, and each choice expands into both branches'
# results. The recursion is only as deep as the tree is high.

from __future__ import annotations

import sys


def make(n: int):
    if n == 0:
        return None
    t = make(n - 1)
    return (t, n, t)


def operator(x: int, y: int) -> int:
    return abs(x - 503 * y + 37) % 1009


def explore(t, s: int):
    if t is None:
        return [s], s
    l, v, r = t
    xs, s2 = explore(l, operator(s, v))
    ys, s4 = explore(r, operator(s2, v))
    return [operator(v, x) for x in xs] + [operator(v, y) for y in ys], s4


def main() -> int:
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 16
    tree = make(n)
    state = 0
    for _ in range(10):
        xs, _ = explore(tree, state)
        state = max([0] + xs)
    print(state)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
