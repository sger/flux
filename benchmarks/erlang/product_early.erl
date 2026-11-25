-module(product_early).
-export([main/0, main/1]).

%% Port of benchmarks/flux/product_early_direct.flx: the product returns 0
%% at the first zero.

main() ->
    run(100000).

main([]) ->
    run(100000);
main([Arg | _]) ->
    run(list_to_integer(Arg)).

run(N) ->
    io:format("~b~n", [loop(lists:seq(1000, 0, -1), N, 0)]).

loop(_, 0, A) -> A;
loop(Xs, I, A) -> loop(Xs, I - 1, A + product(Xs)).

product([]) -> 0;
product([0 | _]) -> 0;
product([Y | Ys]) -> Y * product(Ys).
