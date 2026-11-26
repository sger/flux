defmodule Bench.ParsingDollars do
  # Port of benchmarks/flux/parsing_dollars_direct.flx: each char is made
  # from the two counters where it is read.
  def main(args \\ []) do
    n =
      case args do
        [size | _] -> String.to_integer(size)
        [] -> 20_000
      end

    IO.puts("#{feed(n, 0, 0, 0, 0)}")
  end

  defp feed(n, i, _j, _a, s) when i > n, do: s
  defp feed(n, i, 0, a, s), do: parse(n, i + 1, i + 1, 10, a, s)
  defp feed(n, i, j, a, s), do: parse(n, i, j - 1, 36, a, s)

  defp parse(n, i, j, 36, a, s), do: feed(n, i, j, a + 1, s)
  defp parse(n, i, j, 10, a, s), do: feed(n, i, j, 0, s + a)
  defp parse(_n, _i, _j, _c, _a, s), do: s
end
