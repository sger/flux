#!/usr/bin/env python3

from __future__ import annotations


def wrap32(v: int) -> int:
    m = v % 4294967296
    return m - 4294967296 if m >= 2147483648 else m


def sfib(n: int) -> int:
    x1, x2 = 1, 2
    while n > 0:
        if x2 > 1000000000:
            x1, x2 = 1, 2
        else:
            x1, x2 = wrap32(x2 - x1), wrap32(x1 * x2 + x1)
        n -= 1
    return x1


def main() -> int:
    print(sfib(20000000))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
