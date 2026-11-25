(* Port of benchmarks/flux/countdown.flx with OCaml 5's effect handlers:
   the handler keeps the counter, and both clauses resume as their last
   act. *)
type _ Effect.t += Get : int Effect.t | Set : int -> unit Effect.t

let rec countdown () =
  let i = Effect.perform Get in
  if i = 0 then i
  else begin
    Effect.perform (Set (i - 1));
    countdown ()
  end
;;

let run (n : int) =
  let state = ref n in
  match countdown () with
  | v -> v
  | effect Get, k -> Effect.Deep.continue k !state
  | effect (Set x), k ->
    state := x;
    Effect.Deep.continue k ()
;;

let () =
  let n = try int_of_string (Array.get Sys.argv 1) with _ -> 200000000 in
  Printf.printf "%d\n" (run n)
