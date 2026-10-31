-module(binarytrees).
-export([main/0, main/1]).

%% Port of benchmarks/flux/binarytrees.flx. Node(l, r) is {L, R}; Tip is nil.

main() ->
    run(8).

main([]) ->
    run(8);
main([Arg | _]) ->
    run(list_to_integer(Arg)).

run(N) ->
    MinDepth = 4,
    MaxDepth = max(MinDepth + 2, N),
    StretchDepth = MaxDepth + 1,
    io:format("stretch tree of depth ~b\t check: ~b~n",
              [StretchDepth, check_tree(make_tree(StretchDepth))]),
    LongLived = make_tree(MaxDepth),
    run_depths(MinDepth, MaxDepth, MinDepth),
    io:format("long lived tree of depth ~b\t check: ~b~n",
              [MaxDepth, check_tree(LongLived)]).

make_tree(Depth) when Depth > 0 ->
    {make_tree(Depth - 1), make_tree(Depth - 1)};
make_tree(_) ->
    {nil, nil}.

check_tree({L, R}) -> check_tree(L) + check_tree(R) + 1;
check_tree(nil) -> 0.

sum_trees(Iterations, _Depth, Acc) when Iterations =< 0 ->
    Acc;
sum_trees(Iterations, Depth, Acc) ->
    sum_trees(Iterations - 1, Depth, Acc + check_tree(make_tree(Depth))).

run_depths(Depth, MaxDepth, _MinDepth) when Depth > MaxDepth ->
    ok;
run_depths(Depth, MaxDepth, MinDepth) ->
    Iterations = 1 bsl (MaxDepth + MinDepth - Depth),
    Total = sum_trees(Iterations, Depth, 0),
    io:format("~b\t trees of depth ~b\t check: ~b~n", [Iterations, Depth, Total]),
    run_depths(Depth + 2, MaxDepth, MinDepth).
