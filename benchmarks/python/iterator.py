#!/usr/bin/env python3

# Port of benchmarks/flux/iterator_direct.flx: each emitted value is added
# where it is made, and the recursion is a loop, since Python has no tail
# calls.

from __future__ import annotations

import sys


def range_sum(l: int, u: int) -> int:
    s = 0
    while l <= u:
        s += l
        l += 1
    return s


def main() -> int:
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 40000000
    print(range_sum(0, n))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
