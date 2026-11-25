#!/usr/bin/env python3

# Port of benchmarks/flux/countdown_direct.flx: the state is the loop's
# counter, and the recursion is a loop, since Python has no tail calls.

from __future__ import annotations

import sys


def countdown(i: int) -> int:
    while i != 0:
        i -= 1
    return i


def main() -> int:
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 200000000
    print(countdown(n))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
