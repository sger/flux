-module(tree_explore).
-export([main/0, main/1]).

%% Port of benchmarks/flux/tree_explore_direct.flx: the state is threaded
%% by hand through the branches, and each choice expands into both
%% branches' results.

main() ->
    start(16).

main([]) ->
    start(16);
main([Arg | _]) ->
    start(list_to_integer(Arg)).

start(N) ->
    io:format("~b~n", [turn(10, make(N), 0)]).

make(0) -> leaf;
make(N) ->
    T = make(N - 1),
    {node, T, N, T}.

operator(X, Y) -> abs(X - 503 * Y + 37) rem 1009.

explore(leaf, S) -> {[S], S};
explore({node, L, V, R}, S) ->
    {Xs, S2} = explore(L, operator(S, V)),
    {Ys, S4} = explore(R, operator(S2, V)),
    {[operator(V, X) || X <- Xs] ++ [operator(V, Y) || Y <- Ys], S4}.

turn(0, _T, S) -> S;
turn(I, T, S) ->
    {Xs, _} = explore(T, S),
    turn(I - 1, T, lists:max([0 | Xs])).
