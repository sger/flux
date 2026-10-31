defmodule Bench.Sfib do
  # Elixir integers never overflow, so the 32-bit wrap is explicit.
  def main(_args \\ []) do
    IO.puts("#{sfibx(20_000_000, 1, 2)}")
  end

  defp wrap32(v) do
    m = Bitwise.band(v, 0xFFFFFFFF)
    if m >= 0x80000000, do: m - 0x100000000, else: m
  end

  defp sfibx(n, x1, _x2) when n <= 0, do: x1
  defp sfibx(n, _x1, x2) when x2 > 1_000_000_000, do: sfibx(n - 1, 1, 2)
  defp sfibx(n, x1, x2), do: sfibx(n - 1, wrap32(x2 - x1), wrap32(x1 * x2 + x1))
end
