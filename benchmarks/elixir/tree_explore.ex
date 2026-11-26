defmodule Bench.TreeExplore do
  # Port of benchmarks/flux/tree_explore_direct.flx: the state is threaded
  # by hand through the branches, and each choice expands into both
  # branches' results.
  def main(args \\ []) do
    n =
      case args do
        [size | _] -> String.to_integer(size)
        [] -> 16
      end

    IO.puts("#{turn(10, make(n), 0)}")
  end

  defp make(0), do: :leaf

  defp make(n) do
    t = make(n - 1)
    {:node, t, n, t}
  end

  defp operator(x, y), do: rem(abs(x - 503 * y + 37), 1009)

  defp explore(:leaf, s), do: {[s], s}

  defp explore({:node, l, v, r}, s) do
    {xs, s2} = explore(l, operator(s, v))
    {ys, s4} = explore(r, operator(s2, v))
    {Enum.map(xs, &operator(v, &1)) ++ Enum.map(ys, &operator(v, &1)), s4}
  end

  defp turn(0, _t, s), do: s

  defp turn(i, t, s) do
    {xs, _} = explore(t, s)
    turn(i - 1, t, Enum.max([0 | xs]))
  end
end
