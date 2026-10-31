-module(deriv).
-export([main/0]).

%% Port of benchmarks/flux/deriv.flx.

main() ->
    X = {var, 1},
    nest(0, 10, pow(X, X)).

nest(_Step, 0, _Expr) ->
    ok;
nest(Step, Remaining, Expr) ->
    Next = deriv(1, Expr),
    io:format("~b count: ~b~n", [Step + 1, count(Next)]),
    nest(Step + 1, Remaining - 1, Next).

%% div/rem truncate toward zero, matching Flux's Int division.
pown(_, 0) -> 1;
pown(A, 1) -> A;
pown(A, N) ->
    B = pown(A, N div 2),
    Factor = case N rem 2 of 0 -> 1; _ -> A end,
    B * B * Factor.

add_val_left(N, {val, M}) -> {val, N + M};
add_val_left(0, G) -> G;
add_val_left(N, {add, {val, M}, R}) -> add({val, N + M}, R);
add_val_left(N, {add, L, {val, M}}) -> add({val, N + M}, L);
add_val_left(N, G) -> {add, {val, N}, G}.

add_non_val_left(F, {val, 0}) -> F;
add_non_val_left(F, {val, N}) -> add({val, N}, F);
add_non_val_left(F, {add, {val, N}, R}) -> add({val, N}, add(F, R));
add_non_val_left(F, G) -> {add, F, G}.

add({val, N}, G) -> add_val_left(N, G);
add({add, L, R}, G) -> add(L, add(R, G));
add(F, G) -> add_non_val_left(F, G).

mul_val_left(N, {val, M}) -> {val, N * M};
mul_val_left(0, _) -> {val, 0};
mul_val_left(1, G) -> G;
mul_val_left(N, {mul, {val, M}, R}) -> mul({val, N * M}, R);
mul_val_left(N, {mul, L, {val, M}}) -> mul({val, N * M}, L);
mul_val_left(N, G) -> {mul, {val, N}, G}.

mul_non_val_left(_, {val, 0}) -> {val, 0};
mul_non_val_left(F, {val, 1}) -> F;
mul_non_val_left(F, {val, N}) -> mul({val, N}, F);
mul_non_val_left(F, {mul, {val, N}, R}) -> mul({val, N}, mul(F, R));
mul_non_val_left(F, G) -> {mul, F, G}.

mul({val, N}, G) -> mul_val_left(N, G);
mul({mul, L, R}, G) -> mul(L, mul(R, G));
mul(F, G) -> mul_non_val_left(F, G).

pow({val, M}, {val, N}) -> {val, pown(M, N)};
pow({val, 0}, _) -> {val, 0};
pow({val, _} = F, G) -> {pow, F, G};
pow(_, {val, 0}) -> {val, 1};
pow(F, {val, 1}) -> F;
pow(F, G) -> {pow, F, G}.

ln({val, 1}) -> {val, 0};
ln(F) -> {ln, F}.

deriv(_, {val, _}) -> {val, 0};
deriv(X, {var, X}) -> {val, 1};
deriv(_, {var, _}) -> {val, 0};
deriv(X, {add, F, G}) -> add(deriv(X, F), deriv(X, G));
deriv(X, {mul, F, G}) -> add(mul(F, deriv(X, G)), mul(G, deriv(X, F)));
deriv(X, {pow, F, G}) ->
    mul(pow(F, G),
        add(mul(mul(G, deriv(X, F)), pow(F, {val, -1})),
            mul(ln(F), deriv(X, G))));
deriv(X, {ln, F}) -> mul(deriv(X, F), pow(F, {val, -1})).

count({val, _}) -> 1;
count({var, _}) -> 1;
count({add, F, G}) -> count(F) + count(G);
count({mul, F, G}) -> count(F) + count(G);
count({pow, F, G}) -> count(F) + count(G);
count({ln, F}) -> count(F).
