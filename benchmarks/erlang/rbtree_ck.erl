-module(rbtree_ck).
-export([main/0]).

%% Port of benchmarks/flux/rbtree_ck.flx. Node(c, l, k, v, r) is {C, L, K, V, R}
%% with C = red | black; Leaf is the atom leaf.

-define(LIMIT, 100000).

main() ->
    io:format("~b~n", [count_true(mk_map_aux(?LIMIT, leaf))]).

count_true(leaf) -> 0;
count_true({_, L, _, V, R}) ->
    count_true(L) + (case V of true -> 1; false -> 0 end) + count_true(R).

is_red({red, _, _, _, _}) -> true;
is_red(_) -> false.

balance1({_, _, KV, VV, T}, {_, {red, L, KX, VX, R1}, KY, VY, R2}) ->
    {red, {black, L, KX, VX, R1}, KY, VY, {black, R2, KV, VV, T}};
balance1({_, _, KV, VV, T}, {_, L1, KY, VY, {red, L2, KX, VX, R}}) ->
    {red, {black, L1, KY, VY, L2}, KX, VX, {black, R, KV, VV, T}};
balance1({_, _, KV, VV, T}, {_, L, KY, VY, R}) ->
    {black, {red, L, KY, VY, R}, KV, VV, T};
balance1(_, _) -> leaf.

balance2({_, T, KV, VV, _}, {_, {red, L, KX1, VX1, R1}, KY, VY, R2}) ->
    {red, {black, T, KV, VV, L}, KX1, VX1, {black, R1, KY, VY, R2}};
balance2({_, T, KV, VV, _}, {_, L1, KY, VY, {red, L2, KX2, VX2, R2}}) ->
    {red, {black, T, KV, VV, L1}, KY, VY, {black, L2, KX2, VX2, R2}};
balance2({_, T, KV, VV, _}, {_, L, KY, VY, R}) ->
    {black, T, KV, VV, {red, L, KY, VY, R}};
balance2(_, _) -> leaf.

ins(leaf, KX, VX) ->
    {red, leaf, KX, VX, leaf};
ins({red, A, KY, VY, B}, KX, VX) ->
    if
        KX < KY -> {red, ins(A, KX, VX), KY, VY, B};
        KY < KX -> {red, A, KY, VY, ins(B, KX, VX)};
        true -> {red, A, KY, VY, ins(B, KX, VX)}
    end;
ins({black, A, KY, VY, B}, KX, VX) ->
    if
        KX < KY ->
            case is_red(A) of
                true -> balance1({black, leaf, KY, VY, B}, ins(A, KX, VX));
                false -> {black, ins(A, KX, VX), KY, VY, B}
            end;
        KY < KX ->
            case is_red(B) of
                true -> balance2({black, A, KY, VY, leaf}, ins(B, KX, VX));
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

mk_map_aux(0, T) -> T;
mk_map_aux(N, T) ->
    mk_map_aux(N - 1, insert(T, N, N rem 10 =:= 0)).
