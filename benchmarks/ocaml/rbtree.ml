type color =
| Red
| Black
;;

type tree =
| Leaf
| Node of color * tree * int * bool * tree
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

let balance1 left_tree right_tree =
  match left_tree, right_tree with
  | Node (_, _, kv, vv, tail), Node (_, Node (Red, l, kx, vx, r1), ky, vy, r2) ->
    Node (Red, Node (Black, l, kx, vx, r1), ky, vy, Node (Black, r2, kv, vv, tail))
  | Node (_, _, kv, vv, tail), Node (_, l1, ky, vy, Node (Red, l2, kx, vx, r)) ->
    Node (Red, Node (Black, l1, ky, vy, l2), kx, vx, Node (Black, r, kv, vv, tail))
  | Node (_, _, kv, vv, tail), Node (_, l, ky, vy, r) ->
    Node (Black, Node (Red, l, ky, vy, r), kv, vv, tail)
  | _ -> Leaf
;;

let balance2 left_tree right_tree =
  match left_tree, right_tree with
  | Node (_, tree0, kv, vv, _), Node (_, Node (Red, l, kx1, vx1, r1), ky, vy, r2) ->
    Node (Red, Node (Black, tree0, kv, vv, l), kx1, vx1, Node (Black, r1, ky, vy, r2))
  | Node (_, tree0, kv, vv, _), Node (_, l1, ky, vy, Node (Red, l2, kx2, vx2, r2)) ->
    Node (Red, Node (Black, tree0, kv, vv, l1), ky, vy, Node (Black, l2, kx2, vx2, r2))
  | Node (_, tree0, kv, vv, _), Node (_, l, ky, vy, r) ->
    Node (Black, tree0, kv, vv, Node (Red, l, ky, vy, r))
  | _ -> Leaf
;;

let rec ins tree kx vx =
  match tree with
  | Leaf -> Node (Red, Leaf, kx, vx, Leaf)
  | Node (Red, a, ky, vy, b) ->
    if kx < ky then Node (Red, ins a kx vx, ky, vy, b)
    else if ky < kx then Node (Red, a, ky, vy, ins b kx vx)
    else Node (Red, a, kx, vx, b)
  | Node (Black, a, ky, vy, b) ->
    if kx < ky then
      if is_red a then balance1 (Node (Black, Leaf, ky, vy, b)) (ins a kx vx)
      else Node (Black, ins a kx vx, ky, vy, b)
    else if ky < kx then
      if is_red b then balance2 (Node (Black, a, ky, vy, Leaf)) (ins b kx vx)
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

let rec mk_map_aux n tree =
  if n = 0 then tree
  else
    let n1 = n - 1 in
    let next = insert tree n1 (n1 mod 10 = 0) in
    mk_map_aux n1 next
;;

let () =
  Printf.printf "%d\n" (count_true (mk_map_aux limit Leaf))
