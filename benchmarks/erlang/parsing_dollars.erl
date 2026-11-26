-module(parsing_dollars).
-export([main/0, main/1]).

%% Port of benchmarks/flux/parsing_dollars_direct.flx: each char is made
%% from the two counters where it is read.

main() ->
    start(20000).

main([]) ->
    start(20000);
main([Arg | _]) ->
    start(list_to_integer(Arg)).

start(N) ->
    io:format("~b~n", [feed(N, 0, 0, 0, 0)]).

feed(N, I, _J, _A, S) when I > N -> S;
feed(N, I, 0, A, S) -> parse(N, I + 1, I + 1, 10, A, S);
feed(N, I, J, A, S) -> parse(N, I, J - 1, 36, A, S).

parse(N, I, J, 36, A, S) -> feed(N, I, J, A + 1, S);
parse(N, I, J, 10, A, S) -> feed(N, I, J, 0, S + A);
parse(_N, _I, _J, _C, _A, S) -> S.
