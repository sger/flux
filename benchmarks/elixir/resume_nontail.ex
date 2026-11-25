defmodule Bench.ResumeNontail do
  # Port of benchmarks/flux/resume_nontail_direct.flx: the work after each
  # resume is the work after a call.
  def main(args \\ []) do
    n =
      case args do
        [size | _] -> String.to_integer(size)
        [] -> 10_000
      end

    IO.puts("#{repeat(n, 1000, 0)}")
  end

  defp repeat(_n, 0, s), do: s
  defp repeat(n, l, s), do: repeat(n, l - 1, run(n, s))

  defp run(0, s), do: s
  defp run(i, s), do: combine(i, run(i - 1, s))

  defp combine(x, y), do: rem(abs(x - 503 * y + 37), 1009)
end
