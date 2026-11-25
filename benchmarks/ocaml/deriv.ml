type expr =
| Val of int
| Var of int
| Add of expr * expr
| Mul of expr * expr
| Pow of expr * expr
| Ln of expr
;;

let rec pown a n =
  if n = 0 then 1
  else if n = 1 then a
  else
    let b = pown a (n / 2) in
    b * b * (if n mod 2 = 0 then 1 else a)
;;

let rec add f g =
  match f with
  | Val n ->
    (match g with
     | Val m -> Val (n + m)
     | other when n = 0 -> other
     | Add (Val m, right) -> add (Val (n + m)) right
     | Add (left, Val m) -> add (Val (n + m)) left
     | other -> Add (Val n, other))
  | Add (left, right) -> add left (add right g)
  | _ ->
    (match g with
     | Val 0 -> f
     | Val n -> add (Val n) f
     | Add (Val n, right) -> add (Val n) (add f right)
     | _ -> Add (f, g))
;;

let rec mul f g =
  match f with
  | Val n ->
    (match g with
     | Val m -> Val (n * m)
     | _ when n = 0 -> Val 0
     | other when n = 1 -> other
     | Mul (Val m, right) -> mul (Val (n * m)) right
     | Mul (left, Val m) -> mul (Val (n * m)) left
     | other -> Mul (Val n, other))
  | Mul (left, right) -> mul left (mul right g)
  | _ ->
    (match g with
     | Val 0 -> Val 0
     | Val 1 -> f
     | Val n -> mul (Val n) f
     | Mul (Val n, right) -> mul (Val n) (mul f right)
     | _ -> Mul (f, g))
;;

let pow f g =
  match f with
  | Val m ->
    (match g with
     | Val n -> Val (pown m n)
     | _ when m = 0 -> Val 0
     | _ -> Pow (Val m, g))
  | _ ->
    (match g with
     | Val 0 -> Val 1
     | Val 1 -> f
     | _ -> Pow (f, g))
;;

let ln f =
  match f with
  | Val 1 -> Val 0
  | _ -> Ln f
;;

let d x e =
  let rec deriv e =
    match e with
    | Val _ -> Val 0
    | Var y -> if x = y then Val 1 else Val 0
    | Add (f, g) -> add (deriv f) (deriv g)
    | Mul (f, g) ->
      let df = deriv f in
      let dg = deriv g in
      add (mul f dg) (mul g df)
    | Pow (f, g) ->
      let df = deriv f in
      let dg = deriv g in
      mul
        (pow f g)
        (add
           (mul (mul g df) (pow f (Val (-1))))
           (mul (ln f) dg))
    | Ln f -> mul (deriv f) (pow f (Val (-1)))
  in
  deriv e
;;

let rec count e =
  match e with
  | Val _ -> 1
  | Var _ -> 1
  | Add (f, g) -> count f + count g
  | Mul (f, g) -> count f + count g
  | Pow (f, g) -> count f + count g
  | Ln f -> count f
;;

let rec loop steps i e =
  if i = steps then ()
  else
    let e' = d 1 e in
    Printf.printf "%d count: %d\n" (i + 1) (count e');
    loop steps (i + 1) e'
;;

let () =
  let x = Var 1 in
  let f = pow x x in
  let steps = try int_of_string (Array.get Sys.argv 1) with _ -> 10 in
  loop steps 0 f
