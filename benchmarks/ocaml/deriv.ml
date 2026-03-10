type expr =
| Val of int
| Var of int
| Add of expr * expr
| Mul of expr * expr
| Pow of expr * expr
| Ln of expr
;;

module ExprTbl = Hashtbl.Make (struct
  type t = expr

  let equal a b = a == b
  let hash e = Hashtbl.hash (Obj.repr e)
end)

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
     | e when n = 0 -> e
     | Add (Val m, e) -> add (Val (n + m)) e
     | Add (e, Val m) -> add (Val (n + m)) e
     | Add (e1, e2) -> add (Val n) (add e1 e2)
     | e -> Add (Val n, e))
  | Add (e1, e2) -> add e1 (add e2 g)
  | e ->
    (match g with
     | Val 0 -> e
     | Val n -> add (Val n) e
     | Add (Val n, e2) -> add (Val n) (add e e2)
     | Add (e1, e2) -> add e1 (add e2 e)
     | g' -> Add (e, g'))
;;

let rec mul f g =
  match f with
  | Val n ->
    (match g with
     | Val m -> Val (n * m)
     | _ when n = 0 -> Val 0
     | e when n = 1 -> e
     | Mul (Val m, e) -> mul (Val (n * m)) e
     | Mul (e, Val m) -> mul (Val (n * m)) e
     | Mul (e1, e2) -> mul (Val n) (mul e1 e2)
     | e -> Mul (Val n, e))
  | Mul (e1, e2) -> mul e1 (mul e2 g)
  | e ->
    (match g with
     | Val 0 -> Val 0
     | Val 1 -> e
     | Val n -> mul (Val n) e
     | Mul (Val n, e2) -> mul (Val n) (mul e e2)
     | Mul (e1, e2) -> mul e1 (mul e2 e)
     | g' -> Mul (e, g'))
;;

let pow f g =
  match f with
  | Val m ->
    if m = 0 then Val 0
    else
      (match g with
       | Val n -> Val (pown m n)
       | _ -> Pow (f, g))
  | _ ->
    (match g with
     | Val 0 -> Val 1
     | Val 1 -> f
     | _ ->
       match f with
       | Val 0 -> Val 0
       | _ -> Pow (f, g))
;;

let ln f =
  match f with
  | Val 1 -> Val 0
  | _ -> Ln f
;;

let d x e =
  let cache = ExprTbl.create 1024 in
  let rec deriv e =
    match ExprTbl.find_opt cache e with
    | Some result -> result
    | None ->
      let result =
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
      ExprTbl.add cache e result;
      result
  in
  deriv e
;;

let count e =
  let cache = ExprTbl.create 1024 in
  let rec count_expr e =
    match ExprTbl.find_opt cache e with
    | Some total -> total
    | None ->
      let total =
        match e with
        | Val _ -> 1
        | Var _ -> 1
        | Add (f, g) -> count_expr f + count_expr g
        | Mul (f, g) -> count_expr f + count_expr g
        | Pow (f, g) -> count_expr f + count_expr g
        | Ln f -> count_expr f
      in
      ExprTbl.add cache e total;
      total
  in
  count_expr e
;;

let rec loop i e =
  if i = 10 then ()
  else
    let e' = d 1 e in
    Printf.printf "%d count: %d\n" (i + 1) (count e');
    loop (i + 1) e'
;;

let () =
  let x = Var 1 in
  let f = pow x x in
  loop 0 f
