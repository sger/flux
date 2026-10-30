-module(rbtree_del).
-export([main/0]).

%% Port of benchmarks/flux/rbtree_del.flx. Node(c, l, k, v, r) is
%% {C, L, K, V, R} with C = red | black; Leaf is the atom leaf.
%% Del(t, shrunk) is {T, Shrunk}; Delmin(del, k, v) is {Del, K, V}.

-define(LIMIT, 100000).

main() ->
    io:format("~b~n", [count_true(mk_map_aux(?LIMIT, ?LIMIT, leaf))]).

count_true(leaf) -> 0;
count_true({_, L, _, V, R}) ->
    count_true(L) + (case V of true -> 1; false -> 0 end) + count_true(R).

is_red({red, _, _, _, _}) -> true;
is_red(_) -> false.

balance_left(leaf, _, _, _) ->
    leaf;
balance_left({_, {red, LX, KX, VX, RX}, KY, VY, RY}, K, V, R) ->
    {red, {black, LX, KX, VX, RX}, KY, VY, {black, RY, K, V, R}};
balance_left({_, LY, KY, VY, {red, LX, KX, VX, RX}}, K, V, R) ->
    {red, {black, LY, KY, VY, LX}, KX, VX, {black, RX, K, V, R}};
balance_left({_, LX, KX, VX, RX}, K, V, R) ->
    {black, {red, LX, KX, VX, RX}, K, V, R}.

balance_right(_, _, _, leaf) ->
    leaf;
balance_right(L, K, V, {_, {red, LX, KX, VX, RX}, KY, VY, RY}) ->
    {red, {black, L, K, V, LX}, KX, VX, {black, RX, KY, VY, RY}};
balance_right(L, K, V, {_, LX, KX, VX, {red, LY, KY, VY, RY}}) ->
    {red, {black, L, K, V, LX}, KX, VX, {black, LY, KY, VY, RY}};
balance_right(L, K, V, {_, LX, KX, VX, RX}) ->
    {black, L, K, V, {red, LX, KX, VX, RX}}.

ins(leaf, KX, VX) ->
    {red, leaf, KX, VX, leaf};
ins({red, A, KY, VY, B}, KX, VX) ->
    if
        KX < KY -> {red, ins(A, KX, VX), KY, VY, B};
        true -> {red, A, KY, VY, ins(B, KX, VX)}
    end;
ins({black, A, KY, VY, B}, KX, VX) ->
    if
        KX < KY ->
            case is_red(A) of
                true -> balance_left(ins(A, KX, VX), KY, VY, B);
                false -> {black, ins(A, KX, VX), KY, VY, B}
            end;
        KY < KX ->
            case is_red(B) of
                true -> balance_right(A, KY, VY, ins(B, KX, VX));
                false -> {black, A, KY, VY, ins(B, KX, VX)}
            end;
        true -> {black, A, KX, VX, B}
    end.

set_black({_, L, K, V, R}) -> {black, L, K, V, R};
set_black(T) -> T.

insert(T, K, V) ->
    case is_red(T) of
        true -> set_black(ins(T, K, V));
        false -> ins(T, K, V)
    end.

set_red({_, L, K, V, R}) -> {red, L, K, V, R};
set_red(T) -> T.

make_black({red, L, K, V, R}) -> {{black, L, K, V, R}, false};
make_black(T) -> {T, true}.

rebalance_left(C, {black, _, _, _, _} = L, K, V, R) ->
    {balance_left(set_red(L), K, V, R), C =:= black};
rebalance_left(_, {red, LX, KX, VX, RX}, K, V, R) ->
    {{black, LX, KX, VX, balance_left(set_red(RX), K, V, R)}, false};
rebalance_left(_, _, _, _, _) ->
    {leaf, false}.

rebalance_right(C, L, K, V, {black, _, _, _, _} = R) ->
    {balance_right(L, K, V, set_red(R)), C =:= black};
rebalance_right(_, L, K, V, {red, LX, KX, VX, RX}) ->
    {{black, balance_right(L, K, V, set_red(LX)), KX, VX, RX}, false};
rebalance_right(_, _, _, _, _) ->
    {leaf, false}.

del_min({black, leaf, K, V, leaf}) ->
    {{leaf, true}, K, V};
del_min({black, leaf, K, V, R}) ->
    {{set_black(R), false}, K, V};
del_min({red, leaf, K, V, R}) ->
    {{R, false}, K, V};
del_min({C, L, K, V, R}) ->
    case del_min(L) of
        {{LX, true}, KX, VX} -> {rebalance_right(C, LX, K, V, R), KX, VX};
        {{LX, false}, KX, VX} -> {{{C, LX, K, V, R}, false}, KX, VX}
    end;
del_min(leaf) ->
    {{leaf, false}, 0, false}.

del(leaf, _) ->
    {leaf, false};
del({C, L, KX, VX, R}, Key) when Key < KX ->
    case del(L, Key) of
        {LY, true} -> rebalance_right(C, LY, KX, VX, R);
        {LY, false} -> {{C, LY, KX, VX, R}, false}
    end;
del({C, L, KX, VX, R}, Key) when Key > KX ->
    case del(R, Key) of
        {RY, true} -> rebalance_left(C, L, KX, VX, RY);
        {RY, false} -> {{C, L, KX, VX, RY}, false}
    end;
del({black, L, _, _, leaf}, _) ->
    make_black(L);
del({red, L, _, _, leaf}, _) ->
    {L, false};
del({C, L, _, _, R}, _) ->
    case del_min(R) of
        {{RY, true}, KY, VY} -> rebalance_left(C, L, KY, VY, RY);
        {{RY, false}, KY, VY} -> {{C, L, KY, VY, RY}, false}
    end.

delete(T, Key) ->
    {Next, _} = del(T, Key),
    set_black(Next).

mk_map_aux(_, 0, T) ->
    T;
mk_map_aux(Total, N, T) ->
    N1 = N - 1,
    T1 = insert(T, N1, N1 rem 10 =:= 0),
    case N1 rem 4 of
        0 -> mk_map_aux(Total, N1, delete(T1, N1 + (Total - N1) div 5));
        _ -> mk_map_aux(Total, N1, T1)
    end.
