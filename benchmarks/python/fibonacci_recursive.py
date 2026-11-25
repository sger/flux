#!/usr/bin/env python3

# Port of benchmarks/flux/fibonacci_recursive.flx.

from __future__ import annotations

import sys


def fibonacci(n: int) -> int:
    if n == 0:
        return 0
    if n == 1:
        return 1
    return fibonacci(n - 1) + fibonacci(n - 2)


def main() -> int:
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 42
    print(fibonacci(n))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
