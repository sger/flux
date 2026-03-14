#!/usr/bin/env python3

from __future__ import annotations

N = 32
MASK = 0xFFFFFFFF


def bad_rand(seed: int) -> int:
    return (seed * 1664525 + 1013904223) & MASK


def mk_random_array(seed: int, n: int) -> list[int]:
    xs: list[int] = []
    cur = seed
    for _ in range(n):
        xs.append(cur)
        cur = bad_rand(cur)
    return xs


def check_sorted_checksum(xs: list[int]) -> int:
    if not xs:
        return 0
    for i in range(len(xs) - 1):
        if xs[i] > xs[i + 1]:
            raise RuntimeError("array is not sorted")
    return xs[-1] + len(xs)


def partition(xs: list[int], lo: int, hi: int) -> int:
    mid = (lo + hi) // 2
    if xs[mid] < xs[lo]:
        xs[lo], xs[mid] = xs[mid], xs[lo]
    if xs[hi] < xs[lo]:
        xs[lo], xs[hi] = xs[hi], xs[lo]
    if xs[mid] < xs[hi]:
        xs[mid], xs[hi] = xs[hi], xs[mid]
    pivot = xs[hi]
    i = lo
    for j in range(lo, hi):
        if xs[j] < pivot:
            xs[i], xs[j] = xs[j], xs[i]
            i += 1
    xs[i], xs[hi] = xs[hi], xs[i]
    return i


def qsort_aux(xs: list[int], lo: int, hi: int) -> None:
    if lo < hi:
        mid = partition(xs, lo, hi)
        qsort_aux(xs, lo, mid - 1)
        qsort_aux(xs, mid + 1, hi)


def qsort(xs: list[int]) -> None:
    if xs:
        qsort_aux(xs, 0, len(xs) - 1)


def sort_and_checksum(i: int) -> int:
    xs = mk_random_array(i, i)
    qsort(xs)
    return check_sorted_checksum(xs)


def bench() -> int:
    acc = 0
    for _ in range(N):
        for i in range(N):
            acc += sort_and_checksum(i)
    return acc


def main() -> int:
    print(bench())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
