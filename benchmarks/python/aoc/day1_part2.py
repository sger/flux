#!/usr/bin/env python3
import sys


def count_hits_for_rotation(first_zero_click: int, distance: int) -> int:
    if distance < first_zero_click:
        return 0
    return 1 + (distance - first_zero_click) // 100


def count_zeros_right(pos: int, distance: int) -> int:
    first_zero_click = 100 if pos == 0 else 100 - pos
    return count_hits_for_rotation(first_zero_click, distance)


def count_zeros_left(pos: int, distance: int) -> int:
    first_zero_click = 100 if pos == 0 else pos
    return count_hits_for_rotation(first_zero_click, distance)


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
                hits += count_zeros_left(pos, dist)
                pos = (pos - dist) % 100
            elif direction == "R":
                hits += count_zeros_right(pos, dist)
                pos = (pos + dist) % 100
            else:
                raise ValueError(f"invalid rotation: {line}")

    return hits


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: day1_part2.py <input-file>", file=sys.stderr)
        return 2
    print(solve(sys.argv[1]))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
