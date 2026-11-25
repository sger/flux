-module(fibonacci_recursive).
-export([main/0, main/1]).

%% Port of benchmarks/flux/fibonacci_recursive.flx.

main() ->
    run(42).

main([]) ->
    run(42);
main([Arg | _]) ->
    run(list_to_integer(Arg)).

run(N) ->
    io:format("~b~n", [fibonacci(N)]).

fibonacci(0) -> 0;
fibonacci(1) -> 1;
fibonacci(N) -> fibonacci(N - 1) + fibonacci(N - 2).
