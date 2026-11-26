(* Port of benchmarks/flux/generator.flx with OCaml 5's effect handlers:
   each yield's continuation is kept in a thunk, and the stream is pulled
   to the end. Each continuation is resumed once. *)
type tree = Leaf | Node of tree * int * tree

type generator = Empty | Thunk of int * (unit -> generator)

type _ Effect.t += Yield : int -> unit Effect.t

let rec make n =
  if n = 0 then Leaf
  else
    let t = make (n - 1) in
    Node (t, n, t)
;;

let rec iterate = function
  | Leaf -> ()
  | Node (l, v, r) ->
    iterate l;
    Effect.perform (Yield v);
    iterate r
;;

let generate t =
  match iterate t with
  | () -> Empty
  | effect (Yield v), k -> Thunk (v, fun () -> Effect.Deep.continue k ())
;;

let rec sum a = function
  | Empty -> a
  | Thunk (v, f) -> sum (v + a) (f ())
;;

let run (n : int) = sum 0 (generate (make n))

let () =
  let n = try int_of_string (Array.get Sys.argv 1) with _ -> 25 in
  Printf.printf "%d\n" (run n)
