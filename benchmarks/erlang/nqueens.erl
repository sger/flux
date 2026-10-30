-module(nqueens).
-export([main/0]).

%% Port of benchmarks/flux/nqueens.flx. Solution is a plain list.

main() ->
    io:format("~b~n", [queens(13)]).

safe(_, _, []) -> true;
safe(Queen, Diag, [Q | Tail]) ->
    Queen =/= Q andalso Queen =/= Q + Diag andalso Queen =/= Q - Diag
        andalso safe(Queen, Diag + 1, Tail).

place(_, 0, _) -> 1;
place(N, Row, Soln) -> try_col(N, Row, Soln, N).

try_col(_, _, _, Col) when Col =< 0 -> 0;
try_col(N, Row, Soln, Col) ->
    case safe(Col, 1, Soln) of
        true -> place(N, Row - 1, [Col | Soln]) + try_col(N, Row, Soln, Col - 1);
        false -> try_col(N, Row, Soln, Col - 1)
    end.

queens(N) -> place(N, N, []).
