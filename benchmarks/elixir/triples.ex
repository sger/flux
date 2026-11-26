defmodule Bench.Triples do
  # Port of benchmarks/flux/triples_direct.flx: the triples i > j > k that
  # sum to n, their hashes added as they are found.
  @m 1_000_000_007

  def main(args \\ []) do
    n =
      case args do
        [size | _] -> String.to_integer(size)
        [] -> 10
      end

    IO.puts("#{is(n, n, 0)}")
  end

  defp hash(i, j, k), do: rem(53 * i + 2809 * j + 148_877 * k, @m)

  defp ks(_i, _j, k, _s, acc) when k < 1, do: acc
  defp ks(i, j, k, s, acc) when i + j + k == s, do: ks(i, j, k - 1, s, rem(acc + hash(i, j, k), @m))
  defp ks(i, j, k, s, acc), do: ks(i, j, k - 1, s, acc)

  defp js(_i, j, _s, acc) when j < 1, do: acc
  defp js(i, j, s, acc), do: js(i, j - 1, s, ks(i, j, j - 1, s, acc))

  defp is(i, _s, acc) when i < 1, do: acc
  defp is(i, s, acc), do: is(i - 1, s, js(i, i - 1, s, acc))
end
