-module(triples).
-export([main/0, main/1]).

%% Port of benchmarks/flux/triples_direct.flx: the triples I > J > K that
%% sum to N, their hashes added as they are found.

-define(M, 1000000007).

main() ->
    start(10).

main([]) ->
    start(10);
main([Arg | _]) ->
    start(list_to_integer(Arg)).

start(N) ->
    io:format("~b~n", [is(N, N, 0)]).

hash(I, J, K) -> (53 * I + 2809 * J + 148877 * K) rem ?M.

ks(_I, _J, K, _S, Acc) when K < 1 -> Acc;
ks(I, J, K, S, Acc) when I + J + K == S -> ks(I, J, K - 1, S, (Acc + hash(I, J, K)) rem ?M);
ks(I, J, K, S, Acc) -> ks(I, J, K - 1, S, Acc).

js(_I, J, _S, Acc) when J < 1 -> Acc;
js(I, J, S, Acc) -> js(I, J - 1, S, ks(I, J, J - 1, S, Acc)).

is(I, _S, Acc) when I < 1 -> Acc;
is(I, S, Acc) -> is(I - 1, S, js(I, I - 1, S, Acc)).
