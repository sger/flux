defmodule Bench.Deriv do
  def main(_args \\ []) do
    x = {:var, 1}
    nest(0, 10, pow(x, x))
  end

  defp pown(_a, 0), do: 1
  defp pown(a, 1), do: a

  defp pown(a, n) do
    b = pown(a, div(n, 2))
    factor = if rem(n, 2) == 0, do: 1, else: a
    b * b * factor
  end

  defp add_val_left(n, {:val, m}), do: {:val, n + m}
  defp add_val_left(0, other), do: other
  defp add_val_left(n, {:add, {:val, m}, right}), do: add({:val, n + m}, right)
  defp add_val_left(n, {:add, left, {:val, m}}), do: add({:val, n + m}, left)
  defp add_val_left(n, other), do: {:add, {:val, n}, other}

  defp add_non_val_left(f, {:val, 0}), do: f
  defp add_non_val_left(f, {:val, n}), do: add({:val, n}, f)
  defp add_non_val_left(f, {:add, {:val, n}, right}), do: add({:val, n}, add(f, right))
  defp add_non_val_left(f, g), do: {:add, f, g}

  defp add({:val, n}, g), do: add_val_left(n, g)
  defp add({:add, left, right}, g), do: add(left, add(right, g))
  defp add(f, g), do: add_non_val_left(f, g)

  defp mul_val_left(n, {:val, m}), do: {:val, n * m}
  defp mul_val_left(0, _other), do: {:val, 0}
  defp mul_val_left(1, other), do: other
  defp mul_val_left(n, {:mul, {:val, m}, right}), do: mul({:val, n * m}, right)
  defp mul_val_left(n, {:mul, left, {:val, m}}), do: mul({:val, n * m}, left)
  defp mul_val_left(n, other), do: {:mul, {:val, n}, other}

  defp mul_non_val_left(_f, {:val, 0}), do: {:val, 0}
  defp mul_non_val_left(f, {:val, 1}), do: f
  defp mul_non_val_left(f, {:val, n}), do: mul({:val, n}, f)
  defp mul_non_val_left(f, {:mul, {:val, n}, right}), do: mul({:val, n}, mul(f, right))
  defp mul_non_val_left(f, g), do: {:mul, f, g}

  defp mul({:val, n}, g), do: mul_val_left(n, g)
  defp mul({:mul, left, right}, g), do: mul(left, mul(right, g))
  defp mul(f, g), do: mul_non_val_left(f, g)

  defp pow({:val, m}, {:val, n}), do: {:val, pown(m, n)}
  defp pow({:val, 0}, _other), do: {:val, 0}
  defp pow({:val, _} = f, other), do: {:pow, f, other}
  defp pow(_f, {:val, 0}), do: {:val, 1}
  defp pow(f, {:val, 1}), do: f
  defp pow(f, g), do: {:pow, f, g}

  defp ln({:val, 1}), do: {:val, 0}
  defp ln(f), do: {:ln, f}

  defp deriv(_x, {:val, _}), do: {:val, 0}
  defp deriv(x, {:var, x}), do: {:val, 1}
  defp deriv(_x, {:var, _}), do: {:val, 0}
  defp deriv(x, {:add, f, g}), do: add(deriv(x, f), deriv(x, g))
  defp deriv(x, {:mul, f, g}), do: add(mul(f, deriv(x, g)), mul(g, deriv(x, f)))

  defp deriv(x, {:pow, f, g}) do
    mul(
      pow(f, g),
      add(
        mul(mul(g, deriv(x, f)), pow(f, {:val, -1})),
        mul(ln(f), deriv(x, g))
      )
    )
  end

  defp deriv(x, {:ln, f}), do: mul(deriv(x, f), pow(f, {:val, -1}))

  defp count({:val, _}), do: 1
  defp count({:var, _}), do: 1
  defp count({:add, f, g}), do: count(f) + count(g)
  defp count({:mul, f, g}), do: count(f) + count(g)
  defp count({:pow, f, g}), do: count(f) + count(g)
  defp count({:ln, f}), do: count(f)

  defp nest(_step, 0, _expr), do: :ok

  defp nest(step, remaining, expr) do
    next = deriv(1, expr)
    IO.puts("#{step + 1} count: #{count(next)}")
    nest(step + 1, remaining - 1, next)
  end
end
