#!/usr/bin/env python3

from __future__ import annotations


def tak(x: int, y: int, z: int) -> int:
    if y < x:
        return tak(tak(x - 1, y, z), tak(y - 1, z, x), tak(z - 1, x, y))
    return z


def main() -> int:
    print(tak(30, 20, 10))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
