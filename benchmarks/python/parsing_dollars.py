#!/usr/bin/env python3

# Port of benchmarks/flux/parsing_dollars_direct.flx: each char is made from
# the two counters where it is read, and the two mutually recursive
# functions are one loop, since Python has no tail calls.

from __future__ import annotations

import sys


def run(n: int) -> int:
    i = j = a = s = 0
    while True:
        if i > n:
            return s
        if j == 0:
            i += 1
            j = i
            c = 10
        else:
            j -= 1
            c = 36
        if c == 36:
            a += 1
        elif c == 10:
            s += a
            a = 0
        else:
            return s


def main() -> int:
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 20000
    print(run(n))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
