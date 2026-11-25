defmodule Bench.Counter do
  # Port of benchmarks/flux/counter_direct.flx: the state is the loop's
  # counter, beside the step count.
  def main(args \\ []) do
    n =
      case args do
        [size | _] -> String.to_integer(size)
        [] -> 100_100_100
      end

    IO.puts("#{counter(n, 0)}")
  end

  defp counter(0, c), do: c
  defp counter(i, c), do: counter(i - 1, c + 1)
end
