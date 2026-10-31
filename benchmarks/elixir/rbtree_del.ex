defmodule Bench.RbtreeDel do
  @limit 100_000

  def main(_args \\ []) do
    IO.puts("#{count_true(mk_map_aux(@limit, @limit, :leaf))}")
  end

  defp count_true(:leaf), do: 0

  defp count_true({_, left, _, value, right}),
    do: count_true(left) + if(value, do: 1, else: 0) + count_true(right)

  defp balance_left(:leaf, _k, _v, _r), do: :leaf

  defp balance_left({_, {:red, lx, kx, vx, rx}, ky, vy, ry}, k, v, r),
    do: {:red, {:black, lx, kx, vx, rx}, ky, vy, {:black, ry, k, v, r}}

  defp balance_left({_, ly, ky, vy, {:red, lx, kx, vx, rx}}, k, v, r),
    do: {:red, {:black, ly, ky, vy, lx}, kx, vx, {:black, rx, k, v, r}}

  defp balance_left({_, lx, kx, vx, rx}, k, v, r),
    do: {:black, {:red, lx, kx, vx, rx}, k, v, r}

  defp balance_right(_l, _k, _v, :leaf), do: :leaf

  defp balance_right(l, k, v, {_, {:red, lx, kx, vx, rx}, ky, vy, ry}),
    do: {:red, {:black, l, k, v, lx}, kx, vx, {:black, rx, ky, vy, ry}}

  defp balance_right(l, k, v, {_, lx, kx, vx, {:red, ly, ky, vy, ry}}),
    do: {:red, {:black, l, k, v, lx}, kx, vx, {:black, ly, ky, vy, ry}}

  defp balance_right(l, k, v, {_, lx, kx, vx, rx}),
    do: {:black, l, k, v, {:red, lx, kx, vx, rx}}

  defp ins(:leaf, kx, vx), do: {:red, :leaf, kx, vx, :leaf}

  defp ins({:red, a, ky, vy, b}, kx, vx) when kx < ky, do: {:red, ins(a, kx, vx), ky, vy, b}
  defp ins({:red, a, ky, vy, b}, kx, vx), do: {:red, a, ky, vy, ins(b, kx, vx)}

  defp ins({:black, a, ky, vy, b}, kx, vx) when kx < ky do
    case a do
      {:red, _, _, _, _} -> balance_left(ins(a, kx, vx), ky, vy, b)
      _ -> {:black, ins(a, kx, vx), ky, vy, b}
    end
  end

  defp ins({:black, a, ky, vy, b}, kx, vx) when ky < kx do
    case b do
      {:red, _, _, _, _} -> balance_right(a, ky, vy, ins(b, kx, vx))
      _ -> {:black, a, ky, vy, ins(b, kx, vx)}
    end
  end

  defp ins({:black, a, _ky, _vy, b}, kx, vx), do: {:black, a, kx, vx, b}

  defp set_black({_, left, key, value, right}), do: {:black, left, key, value, right}
  defp set_black(other), do: other

  defp insert({:red, _, _, _, _} = tree, key, value), do: set_black(ins(tree, key, value))
  defp insert(tree, key, value), do: ins(tree, key, value)

  defp set_red({_, left, key, value, right}), do: {:red, left, key, value, right}
  defp set_red(other), do: other

  # Del is {tree, shrunk?}; Delmin is {del, min_key, min_value}.
  defp make_black({:red, left, key, value, right}), do: {{:black, left, key, value, right}, false}
  defp make_black(other), do: {other, true}

  defp rebalance_left(c, {:black, _, _, _, _} = l, k, v, r),
    do: {balance_left(set_red(l), k, v, r), c == :black}

  defp rebalance_left(_c, {:red, lx, kx, vx, rx}, k, v, r),
    do: {{:black, lx, kx, vx, balance_left(set_red(rx), k, v, r)}, false}

  defp rebalance_left(_c, _l, _k, _v, _r), do: {:leaf, false}

  defp rebalance_right(c, l, k, v, {:black, _, _, _, _} = r),
    do: {balance_right(l, k, v, set_red(r)), c == :black}

  defp rebalance_right(_c, l, k, v, {:red, lx, kx, vx, rx}),
    do: {{:black, balance_right(l, k, v, set_red(lx)), kx, vx, rx}, false}

  defp rebalance_right(_c, _l, _k, _v, _r), do: {:leaf, false}

  defp del_min({:black, :leaf, key, value, :leaf}), do: {{:leaf, true}, key, value}
  defp del_min({:black, :leaf, key, value, right}), do: {{set_black(right), false}, key, value}
  defp del_min({:red, :leaf, key, value, right}), do: {{right, false}, key, value}

  defp del_min({color, left, key, value, right}) do
    case del_min(left) do
      {{lx, true}, kx, vx} -> {rebalance_right(color, lx, key, value, right), kx, vx}
      {{lx, false}, kx, vx} -> {{{color, lx, key, value, right}, false}, kx, vx}
    end
  end

  defp del_min(:leaf), do: {{:leaf, false}, 0, false}

  defp del(:leaf, _key), do: {:leaf, false}

  defp del({color, left, kx, vx, right}, key) when key < kx do
    case del(left, key) do
      {ly, true} -> rebalance_right(color, ly, kx, vx, right)
      {ly, false} -> {{color, ly, kx, vx, right}, false}
    end
  end

  defp del({color, left, kx, vx, right}, key) when key > kx do
    case del(right, key) do
      {ry, true} -> rebalance_left(color, left, kx, vx, ry)
      {ry, false} -> {{color, left, kx, vx, ry}, false}
    end
  end

  defp del({:black, left, _kx, _vx, :leaf}, _key), do: make_black(left)
  defp del({_color, left, _kx, _vx, :leaf}, _key), do: {left, false}

  defp del({color, left, _kx, _vx, right}, _key) do
    case del_min(right) do
      {{ry, true}, ky, vy} -> rebalance_left(color, left, ky, vy, ry)
      {{ry, false}, ky, vy} -> {{color, left, ky, vy, ry}, false}
    end
  end

  defp delete(tree, key) do
    {next, _} = del(tree, key)
    set_black(next)
  end

  defp mk_map_aux(_total, 0, tree), do: tree

  defp mk_map_aux(total, n, tree) do
    n1 = n - 1
    t1 = insert(tree, n1, rem(n1, 10) == 0)

    if rem(n1, 4) == 0 do
      mk_map_aux(total, n1, delete(t1, n1 + div(total - n1, 5)))
    else
      mk_map_aux(total, n1, t1)
    end
  end
end
