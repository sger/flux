-module(counter).
-export([main/0, main/1]).

%% Port of benchmarks/flux/counter_direct.flx: the state is the loop's
%% counter, beside the step count.

main() ->
    start(100100100).

main([]) ->
    start(100100100);
main([Arg | _]) ->
    start(list_to_integer(Arg)).

start(N) ->
    io:format("~b~n", [counter(N, 0)]).

counter(0, C) -> C;
counter(I, C) -> counter(I - 1, C + 1).
