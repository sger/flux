defmodule Bench.Tak do
  def main(args \\ []) do
    n =
      case args do
        [size | _] -> String.to_integer(size)
        [] -> 30
      end

    IO.puts("#{tak(n, div(n * 2, 3), div(n, 3))}")
  end

  defp tak(x, y, z) when y < x, do: tak(tak(x - 1, y, z), tak(y - 1, z, x), tak(z - 1, x, y))
  defp tak(_x, _y, z), do: z
end
