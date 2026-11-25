#!/usr/bin/env python3

from __future__ import annotations

import sys


def tak(x: int, y: int, z: int) -> int:
    if y < x:
        return tak(tak(x - 1, y, z), tak(y - 1, z, x), tak(z - 1, x, y))
    return z


def main() -> int:
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 30
    print(tak(n, n * 2 // 3, n // 3))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
