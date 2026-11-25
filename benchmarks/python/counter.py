#!/usr/bin/env python3

# Port of benchmarks/flux/counter_direct.flx: the state is the loop's
# counter, beside the step count, and the recursion is a loop, since
# Python has no tail calls.

from __future__ import annotations

import sys


def counter(i: int) -> int:
    c = 0
    while i != 0:
        i -= 1
        c += 1
    return c


def main() -> int:
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 100100100
    print(counter(n))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
