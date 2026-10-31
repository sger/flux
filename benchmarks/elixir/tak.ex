defmodule Bench.Tak do
  def main(_args \\ []) do
    IO.puts("#{tak(30, 20, 10)}")
  end

  defp tak(x, y, z) when y < x, do: tak(tak(x - 1, y, z), tak(y - 1, z, x), tak(z - 1, x, y))
  defp tak(_x, _y, z), do: z
end
