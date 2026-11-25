(* Port of benchmarks/flux/counter.flx with OCaml 5's effect handlers:
   the handler keeps the state, the loop counts its steps, and both
   clauses resume as their last act. *)
type _ Effect.t += Get : int Effect.t | Set : int -> unit Effect.t

let rec counter c =
  let i = Effect.perform Get in
  if i = 0 then c
  else begin
    Effect.perform (Set (i - 1));
    counter (c + 1)
  end
;;

let run (n : int) =
  let state = ref n in
  match counter 0 with
  | v -> v
  | effect Get, k -> Effect.Deep.continue k !state
  | effect (Set x), k ->
    state := x;
    Effect.Deep.continue k ()
;;

let () =
  let n = try int_of_string (Array.get Sys.argv 1) with _ -> 100100100 in
  Printf.printf "%d\n" (run n)
