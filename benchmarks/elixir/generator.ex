defmodule Bench.Generator do
  # Port of benchmarks/flux/generator_direct.flx: the tree is summed by a
  # walk in order.
  def main(args \\ []) do
    n =
      case args do
        [size | _] -> String.to_integer(size)
        [] -> 25
      end

    IO.puts("#{total(make(n), 0)}")
  end

  defp make(0), do: :leaf

  defp make(n) do
    t = make(n - 1)
    {:node, t, n, t}
  end

  defp total(:leaf, a), do: a
  defp total({:node, l, v, r}, a), do: total(r, total(l, a) + v)
end
