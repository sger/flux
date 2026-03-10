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


def is_black_color(color: str) -> bool:
    return color == BLACK


def balance_left(l, k: int, v: bool, r):
    if l is None:
        return None
    _, left, ky, vy, ry = l
    if left is not None and left[0] == RED:
        _, lx, kx, vx, rx = left
        return node(RED, node(BLACK, lx, kx, vx, rx), ky, vy, node(BLACK, ry, k, v, r))
    if ry is not None and ry[0] == RED:
        _, lx, kx, vx, rx = ry
        return node(RED, node(BLACK, left, ky, vy, lx), kx, vx, node(BLACK, rx, k, v, r))
    return node(BLACK, node(RED, left, ky, vy, ry), k, v, r)


def balance_right(l, k: int, v: bool, r):
    if r is None:
        return None
    _, left, kx, vx, right = r
    if left is not None and left[0] == RED:
        _, lx, ky, vy, rx = left
        return node(RED, node(BLACK, l, k, v, lx), ky, vy, node(BLACK, rx, kx, vx, right))
    if right is not None and right[0] == RED:
        _, ly, ky, vy, ry = right
        return node(RED, node(BLACK, l, k, v, left), kx, vx, node(BLACK, ly, ky, vy, ry))
    return node(BLACK, l, k, v, node(RED, left, kx, vx, right))


def ins(tree, kx: int, vx: bool):
    if tree is None:
        return node(RED, None, kx, vx, None)
    color, a, ky, vy, b = tree
    if color == RED:
        if kx < ky:
            return node(RED, ins(a, kx, vx), ky, vy, b)
        if ky < kx:
            return node(RED, a, ky, vy, ins(b, kx, vx))
        return node(RED, a, ky, vy, ins(b, kx, vx))
    if kx < ky:
        if is_red(a):
            return balance_left(ins(a, kx, vx), ky, vy, b)
        return node(BLACK, ins(a, kx, vx), ky, vy, b)
    if ky < kx:
        if is_red(b):
            return balance_right(a, ky, vy, ins(b, kx, vx))
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


def set_red(tree):
    if tree is None:
        return None
    _, left, key, value, right = tree
    return node(RED, left, key, value, right)


def make_black(tree):
    if tree is not None and tree[0] == RED:
        _, left, key, value, right = tree
        return (node(BLACK, left, key, value, right), False)
    return (tree, True)


def rebalance_left(c, l, k: int, v: bool, r):
    if l is not None and l[0] == BLACK:
        return (balance_left(set_red(l), k, v, r), is_black_color(c))
    if l is not None and l[0] == RED:
        _, lx, kx, vx, rx = l
        return (node(BLACK, lx, kx, vx, balance_left(set_red(rx), k, v, r)), False)
    return (None, False)


def rebalance_right(c, l, k: int, v: bool, r):
    if r is not None and r[0] == BLACK:
        return (balance_right(l, k, v, set_red(r)), is_black_color(c))
    if r is not None and r[0] == RED:
        _, lx, kx, vx, rx = r
        return (node(BLACK, balance_right(l, k, v, set_red(lx)), kx, vx, rx), False)
    return (None, False)


def del_min(tree):
    if tree is None:
        return ((None, False), 0, False)
    color, left, key, value, right = tree
    if color == BLACK and left is None:
        if right is None:
            return ((None, True), key, value)
        return ((set_black(right), False), key, value)
    if color == RED and left is None:
        return ((right, False), key, value)
    (lx, shrunk), kx, vx = del_min(left)
    if shrunk:
        return (rebalance_right(color, lx, key, value, right), kx, vx)
    return ((node(color, lx, key, value, right), False), kx, vx)


def delete_impl(tree, key: int):
    if tree is None:
        return (None, False)
    color, left, kx, vx, right = tree
    if key < kx:
        ly, shrunk = delete_impl(left, key)
        if shrunk:
            return rebalance_right(color, ly, kx, vx, right)
        return (node(color, ly, kx, vx, right), False)
    if key > kx:
        ry, shrunk = delete_impl(right, key)
        if shrunk:
            return rebalance_left(color, left, kx, vx, ry)
        return (node(color, left, kx, vx, ry), False)
    if right is None:
        if is_black_color(color):
            return make_black(left)
        return (left, False)
    (ry, shrunk), ky, vy = del_min(right)
    if shrunk:
        return rebalance_left(color, left, ky, vy, ry)
    return (node(color, left, ky, vy, ry), False)


def delete(tree, key: int):
    tx, _ = delete_impl(tree, key)
    return set_black(tx)


def mk_map_aux(total: int, n: int, tree):
    while n > 0:
        n1 = n - 1
        t1 = insert(tree, n1, n1 % 10 == 0)
        if n1 % 4 == 0:
            tree = delete(t1, n1 + (total - n1) // 5)
        else:
            tree = t1
        n = n1
    return tree


def main() -> int:
    sys.setrecursionlimit(200000)
    print(count_true(mk_map_aux(LIMIT, LIMIT, None)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
