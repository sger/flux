defmodule Bench.Nqueens do
  def main(args \\ []) do
    n =
      case args do
        [size | _] -> String.to_integer(size)
        [] -> 13
      end

    IO.puts("#{queens(n)}")
  end

  defp safe(_queen, _diag, []), do: true

  defp safe(queen, diag, [q | tail]) do
    queen != q and queen != q + diag and queen != q - diag and safe(queen, diag + 1, tail)
  end

  defp place(_n, 0, _soln), do: 1
  defp place(n, row, soln), do: try_col(n, row, soln, n)

  defp try_col(_n, _row, _soln, col) when col <= 0, do: 0

  defp try_col(n, row, soln, col) do
    if safe(col, 1, soln) do
      place(n, row - 1, [col | soln]) + try_col(n, row, soln, col - 1)
    else
      try_col(n, row, soln, col - 1)
    end
  end

  defp queens(n), do: place(n, n, [])
end
