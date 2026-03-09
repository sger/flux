#!/usr/bin/env python3
"""Binary Trees benchmark aligned with examples/perf/binarytrees.flx."""

from __future__ import annotations

import sys

TIP = None


def make_tree(depth: int):
    if depth > 0:
        return (make_tree(depth - 1), make_tree(depth - 1))
    return (TIP, TIP)


def check_tree(tree) -> int:
    if tree is TIP:
        return 0
    left, right = tree
    return check_tree(left) + check_tree(right) + 1


def sum_trees(iterations: int, depth: int) -> int:
    total = 0
    for _ in range(iterations):
        total += check_tree(make_tree(depth))
    return total


def main() -> int:
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 21
    min_depth = 4
    max_depth = max(min_depth + 2, n)
    stretch_depth = max_depth + 1

    print(
        f"stretch tree of depth {stretch_depth}\t check: "
        f"{check_tree(make_tree(stretch_depth))}"
    )

    long_lived_tree = make_tree(max_depth)

    for depth in range(min_depth, max_depth + 1, 2):
        iterations = 1 << (max_depth + min_depth - depth)
        total = sum_trees(iterations, depth)
        print(f"{iterations}\t trees of depth {depth}\t check: {total}")

    print(
        f"long lived tree of depth {max_depth}\t check: "
        f"{check_tree(long_lived_tree)}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
