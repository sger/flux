defmodule Bench.Iterator do
  # Port of benchmarks/flux/iterator_direct.flx: each emitted value is
  # added where it is made.
  def main(args \\ []) do
    n =
      case args do
        [size | _] -> String.to_integer(size)
        [] -> 40_000_000
      end

    IO.puts("#{range(0, n, 0)}")
  end

  defp range(l, u, s) when l > u, do: s
  defp range(l, u, s), do: range(l + 1, u, s + l)
end
