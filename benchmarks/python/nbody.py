#!/usr/bin/env python3

from __future__ import annotations

import math

PI = 3.141592653589793
SOLAR_MASS = 4.0 * PI * PI
DAYS_PER_YEAR = 365.24
DT = 0.01
STEPS = 50000


def planet(x, y, z, vx, vy, vz, mass):
    return [x, y, z, vx * DAYS_PER_YEAR, vy * DAYS_PER_YEAR, vz * DAYS_PER_YEAR, mass * SOLAR_MASS]


def initial():
    return [
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, SOLAR_MASS],
        planet(4.84143144246472090e+00, -1.16032004402742839e+00, -1.03622044471123109e-01,
               1.66007664274403694e-03, 7.69901118419740425e-03, -6.90460016972063023e-05,
               9.54791938424326609e-04),
        planet(8.34336671824457987e+00, 4.12479856412430479e+00, -4.03523417114321381e-01,
               -2.76742510726862411e-03, 4.99852801234917238e-03, 2.30417297573763929e-05,
               2.85885980666130812e-04),
        planet(1.28943695621391310e+01, -1.51111514016986312e+01, -2.23307578892655734e-01,
               2.96460137564761618e-03, 2.37847173959480950e-03, -2.96589568540237556e-05,
               4.36624404335156298e-05),
        planet(1.53796971148509165e+01, -2.59193146099879641e+01, 1.79258772950371181e-01,
               2.68067772490389322e-03, 1.62824170038242295e-03, -9.51592254519715870e-05,
               5.15138902046611451e-05),
    ]


def offset_momentum(bodies) -> None:
    px = py = pz = 0.0
    for b in bodies:
        px += b[3] * b[6]
        py += b[4] * b[6]
        pz += b[5] * b[6]
    sun = bodies[0]
    sun[3] = -px / SOLAR_MASS
    sun[4] = -py / SOLAR_MASS
    sun[5] = -pz / SOLAR_MASS


def energy(bodies) -> float:
    e = 0.0
    for i, b in enumerate(bodies):
        e += 0.5 * b[6] * (b[3] * b[3] + b[4] * b[4] + b[5] * b[5])
        for o in bodies[i + 1:]:
            dx = b[0] - o[0]
            dy = b[1] - o[1]
            dz = b[2] - o[2]
            e -= (b[6] * o[6]) / math.sqrt(dx * dx + dy * dy + dz * dz)
    return e


def advance(bodies) -> None:
    n = len(bodies)
    for i in range(n):
        b = bodies[i]
        for j in range(i + 1, n):
            o = bodies[j]
            dx = b[0] - o[0]
            dy = b[1] - o[1]
            dz = b[2] - o[2]
            d2 = dx * dx + dy * dy + dz * dz
            mag = DT / (d2 * math.sqrt(d2))
            b[3] -= dx * o[6] * mag
            b[4] -= dy * o[6] * mag
            b[5] -= dz * o[6] * mag
            o[3] += dx * b[6] * mag
            o[4] += dy * b[6] * mag
            o[5] += dz * b[6] * mag
    for b in bodies:
        b[0] += DT * b[3]
        b[1] += DT * b[4]
        b[2] += DT * b[5]


def main() -> int:
    bodies = initial()
    offset_momentum(bodies)
    print(round(energy(bodies) * 1.0e9))
    for _ in range(STEPS):
        advance(bodies)
    print(round(energy(bodies) * 1.0e9))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
