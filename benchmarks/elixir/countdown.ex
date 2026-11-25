defmodule Bench.Countdown do
  # Port of benchmarks/flux/countdown_direct.flx: the state is the loop's
  # counter.
  def main(args \\ []) do
    n =
      case args do
        [size | _] -> String.to_integer(size)
        [] -> 200_000_000
      end

    IO.puts("#{countdown(n)}")
  end

  defp countdown(0), do: 0
  defp countdown(i), do: countdown(i - 1)
end
