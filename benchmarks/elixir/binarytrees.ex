defmodule Bench.Binarytrees do
  @min_depth 4

  def main(args \\ []) do
    n =
      case args do
        [depth | _] -> String.to_integer(depth)
        [] -> 8
      end

    max_depth = max(@min_depth + 2, n)
    stretch_depth = max_depth + 1

    IO.puts(
      "stretch tree of depth #{stretch_depth}\t check: #{check_tree(make_tree(stretch_depth))}"
    )

    long_lived_tree = make_tree(max_depth)

    run_depths(@min_depth, max_depth, @min_depth)

    IO.puts("long lived tree of depth #{max_depth}\t check: #{check_tree(long_lived_tree)}")
  end

  defp pow2(exp, acc) when exp <= 0, do: acc
  defp pow2(exp, acc), do: pow2(exp - 1, acc * 2)

  defp make_tree(depth) when depth > 0, do: {make_tree(depth - 1), make_tree(depth - 1)}
  defp make_tree(_depth), do: {:tip, :tip}

  defp check_tree({left, right}), do: check_tree(left) + check_tree(right) + 1
  defp check_tree(:tip), do: 0

  defp sum_trees(iterations, _depth, acc) when iterations <= 0, do: acc

  defp sum_trees(iterations, depth, acc),
    do: sum_trees(iterations - 1, depth, acc + check_tree(make_tree(depth)))

  defp run_depths(depth, max_depth, _min_depth) when depth > max_depth, do: :ok

  defp run_depths(depth, max_depth, min_depth) do
    iterations = pow2(max_depth + min_depth - depth, 1)
    total = sum_trees(iterations, depth, 0)
    IO.puts("#{iterations}\t trees of depth #{depth}\t check: #{total}")
    run_depths(depth + 2, max_depth, min_depth)
  end
end
