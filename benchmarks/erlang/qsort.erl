-module(qsort).
-export([main/0]).

%% Port of benchmarks/flux/qsort.flx. Flux builds Array<Int> values by
%% concatenation; this port uses lists, keeping the first-element pivot and the
%% two separate partition passes (collect_lt / collect_ge).

-define(N, 32).

main() ->
    io:format("~b~n", [bench_outer(?N, 0, 0)]).

bad_rand(Seed) -> (Seed * 1664525 + 1013904223) rem 4294967296.

mk_random_list(_Seed, Size) when Size =< 0 -> [];
mk_random_list(Seed, Size) -> [Seed | mk_random_list(bad_rand(Seed), Size - 1)].

qsort([]) -> [];
qsort([_] = Xs) -> Xs;
qsort([Pivot | Rest]) ->
    Left = [X || X <- Rest, X < Pivot],
    Right = [X || X <- Rest, X >= Pivot],
    qsort(Left) ++ [Pivot | qsort(Right)].

%% Same result as Flux's check_sorted_checksum: 0 for an empty array, -1 if
%% unsorted, otherwise last element + length.
check_sorted_checksum([]) -> 0;
check_sorted_checksum([X | Rest]) -> check_sorted_checksum(X, Rest, 1).

check_sorted_checksum(Last, [], Len) -> Last + Len;
check_sorted_checksum(Prev, [X | Rest], Len) when Prev =< X ->
    check_sorted_checksum(X, Rest, Len + 1);
check_sorted_checksum(_, _, _) -> -1.

sort_and_checksum(I) ->
    check_sorted_checksum(qsort(mk_random_list(I, I))).

bench_inner(Limit, I, Acc) when I < Limit ->
    bench_inner(Limit, I + 1, Acc + sort_and_checksum(I));
bench_inner(_, _, Acc) -> Acc.

bench_outer(Limit, Iter, Acc) when Iter < Limit ->
    bench_outer(Limit, Iter + 1, bench_inner(Limit, 0, Acc));
bench_outer(_, _, Acc) -> Acc.
