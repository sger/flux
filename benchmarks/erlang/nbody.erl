-module(nbody).
-export([main/0]).

%% Port of benchmarks/flux/nbody.flx. A body is {X, Y, Z, VX, VY, VZ, Mass}.

-define(PI, 3.141592653589793).
-define(SOLAR_MASS, (4.0 * ?PI * ?PI)).
-define(DAYS_PER_YEAR, 365.24).
-define(DT, 0.01).
-define(STEPS, 50000).

main() ->
    Bodies = offset_momentum(initial()),
    io:format("~b~n", [round(energy(Bodies) * 1.0e9)]),
    io:format("~b~n", [round(energy(run(Bodies, ?STEPS)) * 1.0e9)]).

planet(X, Y, Z, VX, VY, VZ, Mass) ->
    {X, Y, Z, VX * ?DAYS_PER_YEAR, VY * ?DAYS_PER_YEAR, VZ * ?DAYS_PER_YEAR, Mass * ?SOLAR_MASS}.

initial() ->
    [{0.0, 0.0, 0.0, 0.0, 0.0, 0.0, ?SOLAR_MASS},
     planet(4.84143144246472090e+00, -1.16032004402742839e+00, -1.03622044471123109e-01,
            1.66007664274403694e-03, 7.69901118419740425e-03, -6.90460016972063023e-05,
            9.54791938424326609e-04),
     planet(8.34336671824457987e+00, 4.12479856412430479e+00, -4.03523417114321381e-01,
            -2.76742510726862411e-03, 4.99852801234917238e-03, 2.30417297573763929e-05,
            2.85885980666130812e-04),
     planet(1.28943695621391310e+01, -1.51111514016986312e+01, -2.23307578892655734e-01,
            2.96460137564761618e-03, 2.37847173959480950e-03, -2.96589568540237556e-05,
            4.36624404335156298e-05),
     planet(1.53796971148509165e+01, -2.59193146099879641e+01, 1.79258772950371181e-01,
            2.68067772490389322e-03, 1.62824170038242295e-03, -9.51592254519715870e-05,
            5.15138902046611451e-05)].

offset_momentum([{X, Y, Z, _, _, _, M} | Planets] = Bodies) ->
    {PX, PY, PZ} = lists:foldl(
        fun({_, _, _, VX, VY, VZ, Mass}, {PX, PY, PZ}) ->
            {PX + VX * Mass, PY + VY * Mass, PZ + VZ * Mass}
        end, {0.0, 0.0, 0.0}, Bodies),
    [{X, Y, Z, -PX / ?SOLAR_MASS, -PY / ?SOLAR_MASS, -PZ / ?SOLAR_MASS, M} | Planets].

energy(Bodies) -> energy(Bodies, 0.0).

energy([], E) -> E;
energy([{X, Y, Z, VX, VY, VZ, M} | Rest], E0) ->
    E1 = E0 + 0.5 * M * (VX * VX + VY * VY + VZ * VZ),
    E2 = lists:foldl(
        fun({OX, OY, OZ, _, _, _, OM}, E) ->
            DX = X - OX, DY = Y - OY, DZ = Z - OZ,
            E - (M * OM) / math:sqrt(DX * DX + DY * DY + DZ * DZ)
        end, E1, Rest),
    energy(Rest, E2).

%% Each body sums the pull of every other body in index order, which rounds
%% the same way as the usual loop over pairs.
kick(All, I, {_, _, _, VX, VY, VZ, _} = B) -> kick(All, I, 0, B, VX, VY, VZ).

kick([], _, _, {X, Y, Z, _, _, _, M}, VX, VY, VZ) ->
    {X + ?DT * VX, Y + ?DT * VY, Z + ?DT * VZ, VX, VY, VZ, M};
kick([_ | Rest], I, I, B, VX, VY, VZ) ->
    kick(Rest, I, I + 1, B, VX, VY, VZ);
kick([{OX, OY, OZ, _, _, _, OM} | Rest], I, J, {X, Y, Z, _, _, _, _} = B, VX, VY, VZ) ->
    DX = X - OX, DY = Y - OY, DZ = Z - OZ,
    D2 = DX * DX + DY * DY + DZ * DZ,
    Mag = ?DT / (D2 * math:sqrt(D2)),
    kick(Rest, I, J + 1, B, VX - DX * OM * Mag, VY - DY * OM * Mag, VZ - DZ * OM * Mag).

advance(Bodies) -> advance(Bodies, Bodies, 0).

advance(_, [], _) -> [];
advance(All, [B | Rest], I) -> [kick(All, I, B) | advance(All, Rest, I + 1)].

run(Bodies, 0) -> Bodies;
run(Bodies, N) -> run(advance(Bodies), N - 1).
