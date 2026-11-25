defmodule Bench.NqueensChoice do
  # Port of benchmarks/flux/nqueens_choice_direct.flx: every row is tried in
  # turn instead of by resuming `pick`.
  def main(args \\ []) do
    n =
      case args do
        [size | _] -> String.to_integer(size)
        [] -> 12
      end

    IO.puts("#{place(n, 1, [])}")
  end

  defp place(size, column, _rest) when column > size, do: 1
  defp place(size, column, rest), do: rows(size, column, rest, 1, 0)

  defp rows(size, _column, _rest, row, a) when row > size, do: a

  defp rows(size, column, rest, row, a) do
    found = if safe(row, 1, rest), do: place(size, column + 1, [row | rest]), else: 0
    rows(size, column, rest, row + 1, a + found)
  end

  defp safe(_queen, _diag, []), do: true

  defp safe(queen, diag, [q | qs]) do
    queen != q and queen != q + diag and queen != q - diag and safe(queen, diag + 1, qs)
  end
end
