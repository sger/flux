#!/usr/bin/env python3

from __future__ import annotations

import sys

LIMIT = 100_000
RED = "red"
BLACK = "black"


def node(color, left, key, value, right):
    return (color, left, key, value, right)


def count_true(tree) -> int:
    if tree is None:
        return 0
    _, left, _, value, right = tree
    return count_true(left) + (1 if value else 0) + count_true(right)


def is_red(tree) -> bool:
    return tree is not None and tree[0] == RED


def balance1(left_tree, right_tree):
    if left_tree is None or right_tree is None:
        return None
    _, _, kv, vv, tail = left_tree
    _, left, ky, vy, right = right_tree
    if left is not None and left[0] == RED:
        _, l, kx, vx, r1 = left
        return node(RED, node(BLACK, l, kx, vx, r1), ky, vy, node(BLACK, right, kv, vv, tail))
    if right is not None and right[0] == RED:
        _, l2, kx, vx, r = right
        return node(RED, node(BLACK, left, ky, vy, l2), kx, vx, node(BLACK, r, kv, vv, tail))
    return node(BLACK, node(RED, left, ky, vy, right), kv, vv, tail)


def balance2(left_tree, right_tree):
    if left_tree is None or right_tree is None:
        return None
    _, tree0, kv, vv, _ = left_tree
    _, left, ky, vy, right = right_tree
    if left is not None and left[0] == RED:
        _, l, kx1, vx1, r1 = left
        return node(RED, node(BLACK, tree0, kv, vv, l), kx1, vx1, node(BLACK, r1, ky, vy, right))
    if right is not None and right[0] == RED:
        _, l2, kx2, vx2, r2 = right
        return node(RED, node(BLACK, tree0, kv, vv, left), ky, vy, node(BLACK, l2, kx2, vx2, r2))
    return node(BLACK, tree0, kv, vv, node(RED, left, ky, vy, right))


def ins(tree, kx: int, vx: bool):
    if tree is None:
        return node(RED, None, kx, vx, None)
    color, a, ky, vy, b = tree
    if color == RED:
        if kx < ky:
            return node(RED, ins(a, kx, vx), ky, vy, b)
        if ky < kx:
            return node(RED, a, ky, vy, ins(b, kx, vx))
        return node(RED, a, kx, vx, b)
    if kx < ky:
        if is_red(a):
            return balance1(node(BLACK, None, ky, vy, b), ins(a, kx, vx))
        return node(BLACK, ins(a, kx, vx), ky, vy, b)
    if ky < kx:
        if is_red(b):
            return balance2(node(BLACK, a, ky, vy, None), ins(b, kx, vx))
        return node(BLACK, a, ky, vy, ins(b, kx, vx))
    return node(BLACK, a, kx, vx, b)


def set_black(tree):
    if tree is None:
        return None
    _, left, key, value, right = tree
    return node(BLACK, left, key, value, right)


def insert(tree, key: int, value: bool):
    if is_red(tree):
        return set_black(ins(tree, key, value))
    return ins(tree, key, value)


def mk_map_aux(n: int, tree):
    while n > 0:
        n1 = n - 1
        tree = insert(tree, n1, n1 % 10 == 0)
        n = n1
    return tree


def main() -> int:
    sys.setrecursionlimit(200000)
    print(count_true(mk_map_aux(LIMIT, None)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
