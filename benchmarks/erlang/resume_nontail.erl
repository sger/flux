-module(resume_nontail).
-export([main/0, main/1]).

%% Port of benchmarks/flux/resume_nontail_direct.flx: the work after each
%% resume is the work after a call.

main() ->
    start(10000).

main([]) ->
    start(10000);
main([Arg | _]) ->
    start(list_to_integer(Arg)).

start(N) ->
    io:format("~b~n", [repeat(N, 1000, 0)]).

repeat(_, 0, S) -> S;
repeat(N, L, S) -> repeat(N, L - 1, run(N, S)).

run(0, S) -> S;
run(I, S) -> combine(I, run(I - 1, S)).

combine(X, Y) -> abs(X - 503 * Y + 37) rem 1009.
