#!/usr/bin/env python3

from __future__ import annotations


def safe(queen: int, diag: int, xs) -> bool:
    if xs is None:
        return True
    q, tail = xs
    return queen != q and queen != q + diag and queen != q - diag and safe(queen, diag + 1, tail)


def place(n: int, row: int, soln) -> int:
    if row == 0:
        return 1
    return try_col(n, row, soln, n)


def try_col(n: int, row: int, soln, col: int) -> int:
    if col <= 0:
        return 0
    if safe(col, 1, soln):
        return place(n, row - 1, (col, soln)) + try_col(n, row, soln, col - 1)
    return try_col(n, row, soln, col - 1)


def queens(n: int) -> int:
    return place(n, n, None)


def main() -> int:
    print(queens(13))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
