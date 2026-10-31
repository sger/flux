type body = {
  x : float; y : float; z : float;
  vx : float; vy : float; vz : float;
  mass : float;
}

let pi = 3.141592653589793
let solar_mass = 4.0 *. pi *. pi
let days_per_year = 365.24
let dt = 0.01
let steps = 50000

let planet x y z vx vy vz mass =
  { x; y; z;
    vx = vx *. days_per_year; vy = vy *. days_per_year; vz = vz *. days_per_year;
    mass = mass *. solar_mass }

let initial = [
  { x = 0.0; y = 0.0; z = 0.0; vx = 0.0; vy = 0.0; vz = 0.0; mass = solar_mass };
  planet 4.84143144246472090e+00 (-1.16032004402742839e+00) (-1.03622044471123109e-01)
    1.66007664274403694e-03 7.69901118419740425e-03 (-6.90460016972063023e-05)
    9.54791938424326609e-04;
  planet 8.34336671824457987e+00 4.12479856412430479e+00 (-4.03523417114321381e-01)
    (-2.76742510726862411e-03) 4.99852801234917238e-03 2.30417297573763929e-05
    2.85885980666130812e-04;
  planet 1.28943695621391310e+01 (-1.51111514016986312e+01) (-2.23307578892655734e-01)
    2.96460137564761618e-03 2.37847173959480950e-03 (-2.96589568540237556e-05)
    4.36624404335156298e-05;
  planet 1.53796971148509165e+01 (-2.59193146099879641e+01) 1.79258772950371181e-01
    2.68067772490389322e-03 1.62824170038242295e-03 (-9.51592254519715870e-05)
    5.15138902046611451e-05;
]

let offset_momentum bodies =
  let px, py, pz =
    List.fold_left
      (fun (px, py, pz) b -> (px +. b.vx *. b.mass, py +. b.vy *. b.mass, pz +. b.vz *. b.mass))
      (0.0, 0.0, 0.0) bodies
  in
  match bodies with
  | sun :: planets ->
    { sun with vx = -. px /. solar_mass; vy = -. py /. solar_mass; vz = -. pz /. solar_mass } :: planets
  | [] -> []

let rec energy_from e = function
  | [] -> e
  | b :: rest ->
    let e = e +. 0.5 *. b.mass *. (b.vx *. b.vx +. b.vy *. b.vy +. b.vz *. b.vz) in
    let e =
      List.fold_left
        (fun e o ->
          let dx = b.x -. o.x and dy = b.y -. o.y and dz = b.z -. o.z in
          e -. (b.mass *. o.mass) /. sqrt (dx *. dx +. dy *. dy +. dz *. dz))
        e rest
    in
    energy_from e rest

(* Each body sums the pull of every other body in index order, which rounds
   the same way as the usual loop over pairs. *)
let kick all i b =
  let rec go j vx vy vz = function
    | [] ->
      { b with x = b.x +. dt *. vx; y = b.y +. dt *. vy; z = b.z +. dt *. vz; vx; vy; vz }
    | _ :: rest when j = i -> go (j + 1) vx vy vz rest
    | o :: rest ->
      let dx = b.x -. o.x and dy = b.y -. o.y and dz = b.z -. o.z in
      let d2 = dx *. dx +. dy *. dy +. dz *. dz in
      let mag = dt /. (d2 *. sqrt d2) in
      go (j + 1) (vx -. dx *. o.mass *. mag) (vy -. dy *. o.mass *. mag) (vz -. dz *. o.mass *. mag) rest
  in
  go 0 b.vx b.vy b.vz all

let advance bodies = List.mapi (kick bodies) bodies

let rec run bodies n = if n = 0 then bodies else run (advance bodies) (n - 1)

let () =
  let bodies = offset_momentum initial in
  Printf.printf "%.0f\n" (Float.round (energy_from 0.0 bodies *. 1.0e9));
  Printf.printf "%.0f\n" (Float.round (energy_from 0.0 (run bodies steps) *. 1.0e9))
