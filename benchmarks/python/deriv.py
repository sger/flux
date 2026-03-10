#!/usr/bin/env python3

from __future__ import annotations

import sys

STEPS = 10


def val(n: int):
    return ("val", n)


def var(n: int):
    return ("var", n)


def add_expr(left, right):
    return ("add", left, right)


def mul_expr(left, right):
    return ("mul", left, right)


def pow_expr(left, right):
    return ("pow", left, right)


def ln_expr(expr):
    return ("ln", expr)


def pown(a: int, n: int) -> int:
    if n == 0:
        return 1
    if n == 1:
        return a
    b = pown(a, n // 2)
    return b * b * (1 if n % 2 == 0 else a)


def add(left, right):
    if left[0] == "val":
        n = left[1]
        if right[0] == "val":
            return val(n + right[1])
        if n == 0:
            return right
        if right[0] == "add":
            _, inner_left, inner_right = right
            if inner_left[0] == "val":
                return add(val(n + inner_left[1]), inner_right)
            if inner_right[0] == "val":
                return add(val(n + inner_right[1]), inner_left)
        return add_expr(left, right)
    if left[0] == "add":
        _, inner_left, inner_right = left
        return add(inner_left, add(inner_right, right))
    if right[0] == "val" and right[1] == 0:
        return left
    if right[0] == "val":
        return add(right, left)
    if right[0] == "add":
        _, inner_left, inner_right = right
        if inner_left[0] == "val":
            return add(val(inner_left[1]), add(left, inner_right))
        return add_expr(left, right)
    return add_expr(left, right)


def mul(left, right):
    if left[0] == "val":
        n = left[1]
        if right[0] == "val":
            return val(n * right[1])
        if n == 0:
            return val(0)
        if n == 1:
            return right
        if right[0] == "mul":
            _, inner_left, inner_right = right
            if inner_left[0] == "val":
                return mul(val(n * inner_left[1]), inner_right)
            if inner_right[0] == "val":
                return mul(val(n * inner_right[1]), inner_left)
        return mul_expr(left, right)
    if left[0] == "mul":
        _, inner_left, inner_right = left
        return mul(inner_left, mul(inner_right, right))
    if right[0] == "val" and right[1] == 0:
        return val(0)
    if right[0] == "val" and right[1] == 1:
        return left
    if right[0] == "val":
        return mul(right, left)
    if right[0] == "mul":
        _, inner_left, inner_right = right
        if inner_left[0] == "val":
            return mul(val(inner_left[1]), mul(left, inner_right))
        return mul_expr(left, right)
    return mul_expr(left, right)


def pow_(left, right):
    if left[0] == "val":
        m = left[1]
        if right[0] == "val":
            return val(pown(m, right[1]))
        if m == 0:
            return val(0)
        if right[0] == "val" and right[1] == 0:
            return val(1)
        if right[0] == "val" and right[1] == 1:
            return left
        return pow_expr(left, right)
    if right[0] == "val" and right[1] == 0:
        return val(1)
    if right[0] == "val" and right[1] == 1:
        return left
    if left[0] == "val" and left[1] == 0:
        return val(0)
    return pow_expr(left, right)


def ln(expr):
    if expr[0] == "val" and expr[1] == 1:
        return val(0)
    return ln_expr(expr)


def deriv(x: int, expr):
    tag = expr[0]
    if tag == "val":
        return val(0)
    if tag == "var":
        return val(1 if expr[1] == x else 0)
    if tag == "add":
        _, left, right = expr
        return add(deriv(x, left), deriv(x, right))
    if tag == "mul":
        _, left, right = expr
        return add(mul(left, deriv(x, right)), mul(right, deriv(x, left)))
    if tag == "pow":
        _, left, right = expr
        return mul(
            pow_(left, right),
            add(
                mul(mul(right, deriv(x, left)), pow_(left, val(-1))),
                mul(ln(left), deriv(x, right)),
            ),
        )
    _, inner = expr
    return mul(deriv(x, inner), pow_(inner, val(-1)))


def count(expr) -> int:
    tag = expr[0]
    if tag in ("val", "var"):
        return 1
    if tag == "ln":
        return count(expr[1])
    _, left, right = expr
    return count(left) + count(right)


def main() -> int:
    sys.setrecursionlimit(200000)
    expr = pow_(var(1), var(1))
    for step in range(STEPS):
        expr = deriv(1, expr)
        print(f"{step + 1} count: {count(expr)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
