(* Port of benchmarks/flux/tree_explore_direct.flx: OCaml 5's continuations
   are one-shot, and choose is resumed twice, so the state is threaded by
   hand through the branches and each choice expands into both branches'
   results. *)
type tree = Leaf | Node of tree * int * tree

let rec make n =
  if n = 0 then Leaf
  else
    let t = make (n - 1) in
    Node (t, n, t)
;;

let operator x y = abs (x - (503 * y) + 37) mod 1009

let rec explore t s =
  match t with
  | Leaf -> [ s ], s
  | Node (l, v, r) ->
    let xs, s2 = explore l (operator s v) in
    let ys, s4 = explore r (operator s2 v) in
    List.map (operator v) xs @ List.map (operator v) ys, s4
;;

let rec turn i t s =
  if i = 0 then s
  else
    let xs, _ = explore t s in
    turn (i - 1) t (List.fold_left max 0 xs)
;;

let run (n : int) = turn 10 (make n) 0

let () =
  let n = try int_of_string (Array.get Sys.argv 1) with _ -> 16 in
  Printf.printf "%d\n" (run n)
