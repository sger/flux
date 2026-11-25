defmodule Bench.ProductEarly do
  # Port of benchmarks/flux/product_early_direct.flx: the product returns 0
  # at the first zero.
  def main(args \\ []) do
    n =
      case args do
        [size | _] -> String.to_integer(size)
        [] -> 100_000
      end

    IO.puts("#{loop(Enum.to_list(1000..0//-1), n, 0)}")
  end

  defp loop(_xs, 0, a), do: a
  defp loop(xs, i, a), do: loop(xs, i - 1, a + product(xs))

  defp product([]), do: 0
  defp product([0 | _]), do: 0
  defp product([y | ys]), do: y * product(ys)
end
