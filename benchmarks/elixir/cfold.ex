defmodule Bench.Cfold do
  def main(_args \\ []) do
    original = mk_expr(12, 1)
    optimized = const_folding(reassoc(mk_expr(12, 1)))
    IO.puts("#{eval(original)} #{eval(optimized)}")
  end

  defp dec(0), do: 0
  defp dec(n), do: n - 1

  defp mk_expr(0, 0), do: {:var, 1}
  defp mk_expr(0, v), do: {:val, v}
  defp mk_expr(n, v), do: {:add, mk_expr(n - 1, v + 1), mk_expr(n - 1, dec(v))}

  defp append_add({:add, left, right}, tail), do: {:add, left, append_add(right, tail)}
  defp append_add(expr, tail), do: {:add, expr, tail}

  defp append_mul({:mul, left, right}, tail), do: {:mul, left, append_mul(right, tail)}
  defp append_mul(expr, tail), do: {:mul, expr, tail}

  defp reassoc({:add, left, right}), do: append_add(reassoc(left), reassoc(right))
  defp reassoc({:mul, left, right}), do: append_mul(reassoc(left), reassoc(right))
  defp reassoc(expr), do: expr

  defp fold_add({:val, a}, {:val, b}), do: {:val, a + b}
  defp fold_add({:val, a}, {:add, expr, {:val, b}}), do: {:add, {:val, a + b}, expr}
  defp fold_add({:val, a}, {:add, {:val, b}, expr}), do: {:add, {:val, a + b}, expr}
  defp fold_add(left, right), do: {:add, left, right}

  defp fold_mul({:val, a}, {:val, b}), do: {:val, a * b}
  defp fold_mul({:val, a}, {:mul, expr, {:val, b}}), do: {:mul, {:val, a * b}, expr}
  defp fold_mul({:val, a}, {:mul, {:val, b}, expr}), do: {:mul, {:val, a * b}, expr}
  defp fold_mul(left, right), do: {:mul, left, right}

  defp const_folding({:add, left, right}),
    do: fold_add(const_folding(left), const_folding(right))

  defp const_folding({:mul, left, right}),
    do: fold_mul(const_folding(left), const_folding(right))

  defp const_folding(expr), do: expr

  defp eval({:var, _}), do: 0
  defp eval({:val, value}), do: value
  defp eval({:add, left, right}), do: eval(left) + eval(right)
  defp eval({:mul, left, right}), do: eval(left) * eval(right)
end
