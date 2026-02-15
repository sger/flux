#!/usr/bin/env python3
import sys


def digits_count(n: int) -> int:
    c = 1
    while n >= 10:
        n //= 10
        c += 1
    return c


def invalid_from_seed(seed: int) -> int:
    base = 10 ** digits_count(seed)
    return seed * (base + 1)


def seed_limit_for_max_id(max_id: int) -> int:
    max_digits = digits_count(max_id)
    seed_digits = (max_digits + 1) // 2
    return 10**seed_digits


def solve(path: str) -> int:
    text = open(path, "r", encoding="utf-8").read().strip()
    ranges: list[tuple[int, int]] = []
    max_end = 0
    for token in text.split(","):
        token = token.strip()
        if not token:
            continue
        s, e = token.split("-")
        start = int(s)
        end = int(e)
        ranges.append((start, end))
        if end > max_end:
            max_end = end

    total = 0
    for seed in range(1, seed_limit_for_max_id(max_end)):
        candidate = invalid_from_seed(seed)
        for start, end in ranges:
            if start <= candidate <= end:
                total += candidate
                break
    return total


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: day2_part1.py <input-file>", file=sys.stderr)
        return 2
    print(solve(sys.argv[1]))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
