defmodule Bench.Nbody do
  # A body is {x, y, z, vx, vy, vz, mass}.
  @pi 3.141592653589793
  @solar_mass 4.0 * @pi * @pi
  @days_per_year 365.24
  @dt 0.01
  @steps 50_000

  def main(_args \\ []) do
    bodies = offset_momentum(initial())
    IO.puts("#{round(energy(bodies) * 1.0e9)}")
    IO.puts("#{round(energy(run(bodies, @steps)) * 1.0e9)}")
  end

  defp planet(x, y, z, vx, vy, vz, mass) do
    {x, y, z, vx * @days_per_year, vy * @days_per_year, vz * @days_per_year, mass * @solar_mass}
  end

  defp initial do
    [
      {0.0, 0.0, 0.0, 0.0, 0.0, 0.0, @solar_mass},
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
             5.15138902046611451e-05)
    ]
  end

  defp offset_momentum([{x, y, z, _, _, _, m} | planets] = bodies) do
    {px, py, pz} =
      Enum.reduce(bodies, {0.0, 0.0, 0.0}, fn {_, _, _, vx, vy, vz, mass}, {px, py, pz} ->
        {px + vx * mass, py + vy * mass, pz + vz * mass}
      end)

    [{x, y, z, -px / @solar_mass, -py / @solar_mass, -pz / @solar_mass, m} | planets]
  end

  defp energy(bodies), do: energy(bodies, 0.0)

  defp energy([], e), do: e

  defp energy([{x, y, z, vx, vy, vz, m} | rest], e0) do
    e1 = e0 + 0.5 * m * (vx * vx + vy * vy + vz * vz)

    e2 =
      Enum.reduce(rest, e1, fn {ox, oy, oz, _, _, _, om}, e ->
        dx = x - ox
        dy = y - oy
        dz = z - oz
        e - m * om / :math.sqrt(dx * dx + dy * dy + dz * dz)
      end)

    energy(rest, e2)
  end

  # Each body sums the pull of every other body in index order, which rounds
  # the same way as the usual loop over pairs.
  defp kick(all, i, {_, _, _, vx, vy, vz, _} = b), do: kick(all, i, 0, b, vx, vy, vz)

  defp kick([], _i, _j, {x, y, z, _, _, _, m}, vx, vy, vz) do
    {x + @dt * vx, y + @dt * vy, z + @dt * vz, vx, vy, vz, m}
  end

  defp kick([_ | rest], i, i, b, vx, vy, vz), do: kick(rest, i, i + 1, b, vx, vy, vz)

  defp kick([{ox, oy, oz, _, _, _, om} | rest], i, j, {x, y, z, _, _, _, _} = b, vx, vy, vz) do
    dx = x - ox
    dy = y - oy
    dz = z - oz
    d2 = dx * dx + dy * dy + dz * dz
    mag = @dt / (d2 * :math.sqrt(d2))
    kick(rest, i, j + 1, b, vx - dx * om * mag, vy - dy * om * mag, vz - dz * om * mag)
  end

  defp advance(bodies), do: advance(bodies, bodies, 0)

  defp advance(_all, [], _i), do: []
  defp advance(all, [b | rest], i), do: [kick(all, i, b) | advance(all, rest, i + 1)]

  defp run(bodies, 0), do: bodies
  defp run(bodies, n), do: run(advance(bodies), n - 1)
end
