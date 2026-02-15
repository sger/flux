#!/usr/bin/env python3
import sys


def digits_count(n: int) -> int:
    c = 1
    while n >= 10:
        n //= 10
        c += 1
    return c


def repeat_value(seed: int, base: int, times: int) -> int:
    acc = 0
    for _ in range(times):
        acc = acc * base + seed
    return acc


def has_repeated_pattern_len(value: int, total_len: int, chunk_len: int) -> bool:
    if total_len % chunk_len != 0:
        return False
    repeats = total_len // chunk_len
    if repeats < 2:
        return False
    base = 10 ** chunk_len
    seed = value // (10 ** (total_len - chunk_len))
    return repeat_value(seed, base, repeats) == value


def is_invalid_id(value: int) -> bool:
    total_len = digits_count(value)
    for chunk_len in range(1, total_len // 2 + 1):
        if has_repeated_pattern_len(value, total_len, chunk_len):
            return True
    return False


def solve(path: str) -> int:
    text = open(path, "r", encoding="utf-8").read().strip()
    ranges: list[tuple[int, int]] = []

    for token in text.split(","):
        token = token.strip()
        if not token:
            continue
        s, e = token.split("-")
        start = int(s)
        end = int(e)
        ranges.append((start, end))

    total = 0
    for start, end in ranges:
        for value in range(start, end + 1):
            if is_invalid_id(value):
                total += value
    return total


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: day2_part2.py <input-file>", file=sys.stderr)
        return 2
    print(solve(sys.argv[1]))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
