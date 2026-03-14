type color =
| Red
| Black
;;

type tree =
| Leaf
| Node of color * tree * int * bool * tree
;;

type del = Del of tree * bool
;;

type delmin = Delmin of del * int * bool
;;

let limit = 100000
;;

let rec count_true tree =
  match tree with
  | Leaf -> 0
  | Node (_, left, _, value, right) ->
    count_true left + (if value then 1 else 0) + count_true right
;;

let is_red tree =
  match tree with
  | Node (Red, _, _, _, _) -> true
  | _ -> false
;;

let is_black_color color =
  match color with
  | Black -> true
  | _ -> false
;;

let balance_left l k v r =
  match l with
  | Leaf -> Leaf
  | Node (_, Node (Red, lx, kx, vx, rx), ky, vy, ry) ->
    Node (Red, Node (Black, lx, kx, vx, rx), ky, vy, Node (Black, ry, k, v, r))
  | Node (_, ly, ky, vy, Node (Red, lx, kx, vx, rx)) ->
    Node (Red, Node (Black, ly, ky, vy, lx), kx, vx, Node (Black, rx, k, v, r))
  | Node (_, lx, kx, vx, rx) ->
    Node (Black, Node (Red, lx, kx, vx, rx), k, v, r)
;;

let balance_right l k v r =
  match r with
  | Leaf -> Leaf
  | Node (_, Node (Red, lx, kx, vx, rx), ky, vy, ry) ->
    Node (Red, Node (Black, l, k, v, lx), kx, vx, Node (Black, rx, ky, vy, ry))
  | Node (_, lx, kx, vx, Node (Red, ly, ky, vy, ry)) ->
    Node (Red, Node (Black, l, k, v, lx), kx, vx, Node (Black, ly, ky, vy, ry))
  | Node (_, lx, kx, vx, rx) ->
    Node (Black, l, k, v, Node (Red, lx, kx, vx, rx))
;;

let rec ins tree kx vx =
  match tree with
  | Leaf -> Node (Red, Leaf, kx, vx, Leaf)
  | Node (Red, a, ky, vy, b) ->
    if kx < ky then Node (Red, ins a kx vx, ky, vy, b)
    else if ky < kx then Node (Red, a, ky, vy, ins b kx vx)
    else Node (Red, a, ky, vy, ins b kx vx)
  | Node (Black, a, ky, vy, b) ->
    if kx < ky then
      if is_red a then balance_left (ins a kx vx) ky vy b
      else Node (Black, ins a kx vx, ky, vy, b)
    else if ky < kx then
      if is_red b then balance_right a ky vy (ins b kx vx)
      else Node (Black, a, ky, vy, ins b kx vx)
    else Node (Black, a, kx, vx, b)
;;

let set_black tree =
  match tree with
  | Node (_, left, key, value, right) -> Node (Black, left, key, value, right)
  | other -> other
;;

let insert tree key value =
  if is_red tree then set_black (ins tree key value)
  else ins tree key value
;;

let set_red tree =
  match tree with
  | Node (_, left, key, value, right) -> Node (Red, left, key, value, right)
  | other -> other
;;

let make_black tree =
  match tree with
  | Node (Red, left, key, value, right) -> Del (Node (Black, left, key, value, right), false)
  | other -> Del (other, true)
;;

let rebalance_left c l k v r =
  match l with
  | Node (Black, _, _, _, _) -> Del (balance_left (set_red l) k v r, is_black_color c)
  | Node (Red, lx, kx, vx, rx) -> Del (Node (Black, lx, kx, vx, balance_left (set_red rx) k v r), false)
  | _ -> Del (Leaf, false)
;;

let rebalance_right c l k v r =
  match r with
  | Node (Black, _, _, _, _) -> Del (balance_right l k v (set_red r), is_black_color c)
  | Node (Red, lx, kx, vx, rx) -> Del (Node (Black, balance_right l k v (set_red lx), kx, vx, rx), false)
  | _ -> Del (Leaf, false)
;;

let rec del_min tree =
  match tree with
  | Node (Black, Leaf, key, value, right) ->
    (match right with
     | Leaf -> Delmin (Del (Leaf, true), key, value)
     | _ -> Delmin (Del (set_black right, false), key, value))
  | Node (Red, Leaf, key, value, right) ->
    Delmin (Del (right, false), key, value)
  | Node (color, left, key, value, right) ->
    (match del_min left with
     | Delmin (Del (lx, true), kx, vx) -> Delmin (rebalance_right color lx key value right, kx, vx)
     | Delmin (Del (lx, false), kx, vx) -> Delmin (Del (Node (color, lx, key, value, right), false), kx, vx))
  | Leaf -> Delmin (Del (Leaf, false), 0, false)
;;

let rec del tree key =
  match tree with
  | Leaf -> Del (Leaf, false)
  | Node (color, left, kx, vx, right) ->
    if key < kx then
      (match del left key with
       | Del (ly, true) -> rebalance_right color ly kx vx right
       | Del (ly, false) -> Del (Node (color, ly, kx, vx, right), false))
    else if key > kx then
      (match del right key with
       | Del (ry, true) -> rebalance_left color left kx vx ry
       | Del (ry, false) -> Del (Node (color, left, kx, vx, ry), false))
    else
      match right with
      | Leaf -> if is_black_color color then make_black left else Del (left, false)
      | _ ->
        (match del_min right with
         | Delmin (Del (ry, true), ky, vy) -> rebalance_left color left ky vy ry
         | Delmin (Del (ry, false), ky, vy) -> Del (Node (color, left, ky, vy, ry), false))
;;

let delete tree key =
  match del tree key with
  | Del (next, _) -> set_black next
;;

let rec mk_map_aux total n tree =
  if n = 0 then tree
  else
    let n1 = n - 1 in
    let t1 = insert tree n1 (n1 mod 10 = 0) in
    let t2 = if n1 mod 4 = 0 then delete t1 (n1 + (total - n1) / 5) else t1 in
    mk_map_aux total n1 t2
;;

let () =
  Printf.printf "%d\n" (count_true (mk_map_aux limit limit Leaf))
