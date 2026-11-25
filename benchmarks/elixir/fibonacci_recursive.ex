defmodule Bench.FibonacciRecursive do
  # Port of benchmarks/flux/fibonacci_recursive.flx.
  def main(args \\ []) do
    n =
      case args do
        [size | _] -> String.to_integer(size)
        [] -> 42
      end

    IO.puts("#{fibonacci(n)}")
  end

  defp fibonacci(0), do: 0
  defp fibonacci(1), do: 1
  defp fibonacci(n), do: fibonacci(n - 1) + fibonacci(n - 2)
end
