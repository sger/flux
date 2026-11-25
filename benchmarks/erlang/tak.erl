-module(tak).
-export([main/0, main/1]).

%% Port of benchmarks/flux/tak.flx.

main() ->
    run(30).

main([]) ->
    run(30);
main([Arg | _]) ->
    run(list_to_integer(Arg)).

run(N) ->
    io:format("~b~n", [tak(N, N * 2 div 3, N div 3)]).

tak(X, Y, Z) when Y < X ->
    tak(tak(X - 1, Y, Z), tak(Y - 1, Z, X), tak(Z - 1, X, Y));
tak(_, _, Z) -> Z.
