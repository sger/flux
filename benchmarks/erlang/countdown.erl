-module(countdown).
-export([main/0, main/1]).

%% Port of benchmarks/flux/countdown_direct.flx: the state is the loop's
%% counter.

main() ->
    start(200000000).

main([]) ->
    start(200000000);
main([Arg | _]) ->
    start(list_to_integer(Arg)).

start(N) ->
    io:format("~b~n", [countdown(N)]).

countdown(0) -> 0;
countdown(I) -> countdown(I - 1).
