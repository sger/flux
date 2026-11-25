-module(iterator).
-export([main/0, main/1]).

%% Port of benchmarks/flux/iterator_direct.flx: each emitted value is
%% added where it is made.

main() ->
    start(40000000).

main([]) ->
    start(40000000);
main([Arg | _]) ->
    start(list_to_integer(Arg)).

start(N) ->
    io:format("~b~n", [range(0, N, 0)]).

range(L, U, S) when L > U -> S;
range(L, U, S) -> range(L + 1, U, S + L).
