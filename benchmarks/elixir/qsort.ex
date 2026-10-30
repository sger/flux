defmodule Bench.Qsort do
  @n 32
  @rand_mod 4_294_967_296

  def main(_args \\ []) do
    IO.puts("#{bench_outer(@n, 0, 0)}")
  end

  defp bad_rand(seed), do: rem(seed * 1_664_525 + 1_013_904_223, @rand_mod)

  defp mk_random_array(_seed, size) when size <= 0, do: []
  defp mk_random_array(seed, size), do: [seed | mk_random_array(bad_rand(seed), size - 1)]

  defp collect_lt([], _pivot), do: []
  defp collect_lt([x | xs], pivot) when x < pivot, do: [x | collect_lt(xs, pivot)]
  defp collect_lt([_ | xs], pivot), do: collect_lt(xs, pivot)

  defp collect_ge([], _pivot), do: []
  defp collect_ge([x | xs], pivot) when x < pivot, do: collect_ge(xs, pivot)
  defp collect_ge([x | xs], pivot), do: [x | collect_ge(xs, pivot)]

  defp qsort([]), do: []
  defp qsort([_] = xs), do: xs

  defp qsort([pivot | rest]) do
    qsort(collect_lt(rest, pivot)) ++ [pivot | qsort(collect_ge(rest, pivot))]
  end

  defp check_sorted_checksum([]), do: 0
  defp check_sorted_checksum(xs), do: check_sorted_checksum(xs, 1)

  defp check_sorted_checksum([a, b | rest], len) when a <= b,
    do: check_sorted_checksum([b | rest], len + 1)

  defp check_sorted_checksum([_, _ | _], _len), do: -1
  defp check_sorted_checksum([last], len), do: last + len

  defp sort_and_checksum(i), do: check_sorted_checksum(qsort(mk_random_array(i, i)))

  defp bench_inner(limit, i, acc) when i < limit,
    do: bench_inner(limit, i + 1, acc + sort_and_checksum(i))

  defp bench_inner(_limit, _i, acc), do: acc

  defp bench_outer(limit, iter, acc) when iter < limit,
    do: bench_outer(limit, iter + 1, bench_inner(limit, 0, acc))

  defp bench_outer(_limit, _iter, acc), do: acc
end
