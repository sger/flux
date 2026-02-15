#!/usr/bin/env python3
import sys


def solve(path: str) -> int:
    pos = 50
    hits = 0

    with open(path, "r", encoding="utf-8") as f:
        for raw in f:
            line = raw.strip()
            if not line:
                continue
            direction = line[0]
            dist = int(line[1:])
            if direction == "L":
                pos = (pos - dist) % 100
            elif direction == "R":
                pos = (pos + dist) % 100
            else:
                raise ValueError(f"invalid rotation: {line}")
            if pos == 0:
                hits += 1

    return hits


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: day1.py <input-file>", file=sys.stderr)
        return 2
    print(solve(sys.argv[1]))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
