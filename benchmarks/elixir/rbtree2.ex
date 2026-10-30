defmodule Bench.Rbtree2 do
  @limit 100_000

  def main(_args \\ []) do
    IO.puts("#{count_true(mk_map_aux(@limit, :leaf))}")
  end

  defp count_true(:leaf), do: 0

  defp count_true({_, left, _, value, right}),
    do: count_true(left) + if(value, do: 1, else: 0) + count_true(right)

  defp balance1({_, _, kv, vv, tail}, {_, {:red, l, kx, vx, r1}, ky, vy, r2}),
    do: {:red, {:black, l, kx, vx, r1}, ky, vy, {:black, r2, kv, vv, tail}}

  defp balance1({_, _, kv, vv, tail}, {_, l1, ky, vy, {:red, l2, kx, vx, r}}),
    do: {:red, {:black, l1, ky, vy, l2}, kx, vx, {:black, r, kv, vv, tail}}

  defp balance1({_, _, kv, vv, tail}, {_, l, ky, vy, r}),
    do: {:black, {:red, l, ky, vy, r}, kv, vv, tail}

  defp balance1(_, _), do: :leaf

  defp balance2({_, tree0, kv, vv, _}, {_, {:red, l, kx1, vx1, r1}, ky, vy, r2}),
    do: {:red, {:black, tree0, kv, vv, l}, kx1, vx1, {:black, r1, ky, vy, r2}}

  defp balance2({_, tree0, kv, vv, _}, {_, l1, ky, vy, {:red, l2, kx2, vx2, r2}}),
    do: {:red, {:black, tree0, kv, vv, l1}, ky, vy, {:black, l2, kx2, vx2, r2}}

  defp balance2({_, tree0, kv, vv, _}, {_, l, ky, vy, r}),
    do: {:black, tree0, kv, vv, {:red, l, ky, vy, r}}

  defp balance2(_, _), do: :leaf

  defp ins(:leaf, kx, vx), do: {:red, :leaf, kx, vx, :leaf}

  defp ins({:red, a, ky, vy, b}, kx, vx) when kx < ky, do: {:red, ins(a, kx, vx), ky, vy, b}
  defp ins({:red, a, ky, vy, b}, kx, vx) when ky < kx, do: {:red, a, ky, vy, ins(b, kx, vx)}
  defp ins({:red, a, ky, vy, b}, kx, vx), do: {:red, a, ky, vy, ins(b, kx, vx)}

  defp ins({:black, a, ky, vy, b}, kx, vx) when kx < ky do
    case a do
      {:red, _, _, _, _} -> balance1({:black, :leaf, ky, vy, b}, ins(a, kx, vx))
      _ -> {:black, ins(a, kx, vx), ky, vy, b}
    end
  end

  defp ins({:black, a, ky, vy, b}, kx, vx) when ky < kx do
    case b do
      {:red, _, _, _, _} -> balance2({:black, a, ky, vy, :leaf}, ins(b, kx, vx))
      _ -> {:black, a, ky, vy, ins(b, kx, vx)}
    end
  end

  defp ins({:black, a, _ky, _vy, b}, kx, vx), do: {:black, a, kx, vx, b}

  defp set_black({_, left, key, value, right}), do: {:black, left, key, value, right}

  defp insert({:red, _, _, _, _} = tree, key, value), do: set_black(ins(tree, key, value))

  defp insert(tree, key, value), do: ins(tree, key, value)

  defp mk_map_aux(0, tree), do: tree

  defp mk_map_aux(n, tree) do
    n1 = n - 1
    mk_map_aux(n1, insert(tree, n1, rem(n1, 10) == 0))
  end
end
