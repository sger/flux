-module(cfold).
-export([main/0]).

%% Port of benchmarks/flux/cfold.flx.

main() ->
    Original = mk_expr(12, 1),
    Optimized = const_folding(reassoc(mk_expr(12, 1))),
    io:format("~b ~b~n", [eval(Original), eval(Optimized)]).

dec(0) -> 0;
dec(N) -> N - 1.

mk_expr(0, 0) -> {var, 1};
mk_expr(0, V) -> {val, V};
mk_expr(N, V) -> {add, mk_expr(N - 1, V + 1), mk_expr(N - 1, dec(V))}.

append_add({add, L, R}, Tail) -> {add, L, append_add(R, Tail)};
append_add(E, Tail) -> {add, E, Tail}.

append_mul({mul, L, R}, Tail) -> {mul, L, append_mul(R, Tail)};
append_mul(E, Tail) -> {mul, E, Tail}.

reassoc({add, L, R}) -> append_add(reassoc(L), reassoc(R));
reassoc({mul, L, R}) -> append_mul(reassoc(L), reassoc(R));
reassoc(E) -> E.

fold_add({val, A}, {val, B}) -> {val, A + B};
fold_add({val, A}, {add, E, {val, B}}) -> {add, {val, A + B}, E};
fold_add({val, A}, {add, {val, B}, E}) -> {add, {val, A + B}, E};
fold_add(L, R) -> {add, L, R}.

fold_mul({val, A}, {val, B}) -> {val, A * B};
fold_mul({val, A}, {mul, E, {val, B}}) -> {mul, {val, A * B}, E};
fold_mul({val, A}, {mul, {val, B}, E}) -> {mul, {val, A * B}, E};
fold_mul(L, R) -> {mul, L, R}.

const_folding({add, L, R}) -> fold_add(const_folding(L), const_folding(R));
const_folding({mul, L, R}) -> fold_mul(const_folding(L), const_folding(R));
const_folding(E) -> E.

eval({var, _}) -> 0;
eval({val, V}) -> V;
eval({add, L, R}) -> eval(L) + eval(R);
eval({mul, L, R}) -> eval(L) * eval(R).
