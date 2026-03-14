#!/usr/bin/env python3

from __future__ import annotations

import sys

DEPTH = 12


def var(value: int) -> tuple[str, int]:
    return ("var", value)


def val(value: int) -> tuple[str, int]:
    return ("val", value)


def add(left, right):
    return ("add", left, right)


def mul(left, right):
    return ("mul", left, right)


def dec(n: int) -> int:
    return 0 if n == 0 else n - 1


def mk_expr(n: int, v: int):
    if n == 0:
        return var(1) if v == 0 else val(v)
    return add(mk_expr(n - 1, v + 1), mk_expr(n - 1, dec(v)))


def append_add(expr, tail):
    left_nodes = []
    while expr[0] == "add":
        _, left, right = expr
        left_nodes.append(left)
        expr = right
    result = add(expr, tail)
    while left_nodes:
        result = add(left_nodes.pop(), result)
    return result


def append_mul(expr, tail):
    left_nodes = []
    while expr[0] == "mul":
        _, left, right = expr
        left_nodes.append(left)
        expr = right
    result = mul(expr, tail)
    while left_nodes:
        result = mul(left_nodes.pop(), result)
    return result


def reassoc(expr):
    tag = expr[0]
    if tag == "add":
        _, left, right = expr
        return append_add(reassoc(left), reassoc(right))
    if tag == "mul":
        _, left, right = expr
        return append_mul(reassoc(left), reassoc(right))
    return expr


def const_folding(expr):
    tag = expr[0]
    if tag == "add":
        _, left, right = expr
        left_folded = const_folding(left)
        right_folded = const_folding(right)
        return fold_add(left_folded, right_folded)
    if tag == "mul":
        _, left, right = expr
        left_folded = const_folding(left)
        right_folded = const_folding(right)
        return fold_mul(left_folded, right_folded)
    return expr


def fold_add(left, right):
    if left[0] == "val" and right[0] == "val":
        return val(left[1] + right[1])
    if left[0] == "val" and right[0] == "add":
        _, inner_left, inner_right = right
        if inner_right[0] == "val":
            return add(val(left[1] + inner_right[1]), inner_left)
        if inner_left[0] == "val":
            return add(val(left[1] + inner_left[1]), inner_right)
    return add(left, right)


def fold_mul(left, right):
    if left[0] == "val" and right[0] == "val":
        return val(left[1] * right[1])
    if left[0] == "val" and right[0] == "mul":
        _, inner_left, inner_right = right
        if inner_right[0] == "val":
            return mul(val(left[1] * inner_right[1]), inner_left)
        if inner_left[0] == "val":
            return mul(val(left[1] * inner_left[1]), inner_right)
    return mul(left, right)


def eval_expr(expr) -> int:
    tag = expr[0]
    if tag == "var":
        return 0
    if tag == "val":
        return expr[1]
    _, left, right = expr
    if tag == "add":
        return eval_expr(left) + eval_expr(right)
    return eval_expr(left) * eval_expr(right)


def main() -> int:
    sys.setrecursionlimit(20000)
    original = mk_expr(DEPTH, 1)
    optimized = const_folding(reassoc(mk_expr(DEPTH, 1)))
    print(f"{eval_expr(original)} {eval_expr(optimized)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
