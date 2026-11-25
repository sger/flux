-module(nqueens_choice).
-export([main/0, main/1]).

%% Port of benchmarks/flux/nqueens_choice_direct.flx: every row is tried in
%% turn instead of by resuming `pick`.

main() ->
    run(12).

main([]) ->
    run(12);
main([Arg | _]) ->
    run(list_to_integer(Arg)).

run(N) ->
    io:format("~b~n", [place(N, 1, [])]).

place(Size, Column, _) when Column > Size -> 1;
place(Size, Column, Rest) -> rows(Size, Column, Rest, 1, 0).

rows(Size, _, _, Row, A) when Row > Size -> A;
rows(Size, Column, Rest, Row, A) ->
    Found = case safe(Row, 1, Rest) of
        true -> place(Size, Column + 1, [Row | Rest]);
        false -> 0
    end,
    rows(Size, Column, Rest, Row + 1, A + Found).

safe(_, _, []) -> true;
safe(Queen, Diag, [Q | Qs]) ->
    Queen =/= Q andalso Queen =/= Q + Diag andalso Queen =/= Q - Diag
        andalso safe(Queen, Diag + 1, Qs).
