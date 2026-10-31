-module(tak).
-export([main/0]).

%% Port of benchmarks/flux/tak.flx.

main() ->
    io:format("~b~n", [tak(30, 20, 10)]).

tak(X, Y, Z) when Y < X ->
    tak(tak(X - 1, Y, Z), tak(Y - 1, Z, X), tak(Z - 1, X, Y));
tak(_, _, Z) -> Z.
