-module(sfib).
-export([main/0, main/1]).

%% Port of benchmarks/flux/sfib.flx. Erlang integers never overflow, so the
%% 32-bit wrap is explicit.

main() ->
    run(20000000).

main([]) ->
    run(20000000);
main([Arg | _]) ->
    run(list_to_integer(Arg)).

run(N) ->
    io:format("~b~n", [sfibx(N, 1, 2)]).

wrap32(V) ->
    M = V band 16#FFFFFFFF,
    if
        M >= 16#80000000 -> M - 16#100000000;
        true -> M
    end.

sfibx(N, X1, _) when N =< 0 -> X1;
sfibx(N, _, X2) when X2 > 1000000000 -> sfibx(N - 1, 1, 2);
sfibx(N, X1, X2) -> sfibx(N - 1, wrap32(X2 - X1), wrap32(X1 * X2 + X1)).
