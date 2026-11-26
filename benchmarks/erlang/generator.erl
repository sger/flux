-module(generator).
-export([main/0, main/1]).

%% Port of benchmarks/flux/generator_direct.flx: the tree is summed by a
%% walk in order.

main() ->
    start(25).

main([]) ->
    start(25);
main([Arg | _]) ->
    start(list_to_integer(Arg)).

start(N) ->
    io:format("~b~n", [total(make(N), 0)]).

make(0) -> leaf;
make(N) ->
    T = make(N - 1),
    {node, T, N, T}.

total(leaf, A) -> A;
total({node, L, V, R}, A) -> total(R, total(L, A) + V).
