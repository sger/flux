(* Port of benchmarks/flux/iterator.flx with OCaml 5's effect handlers:
   each emitted value is added into the handler's sum, and the clause
   resumes as its last act. *)
type _ Effect.t += Emit : int -> unit Effect.t

let rec range l u =
  if l > u then ()
  else begin
    Effect.perform (Emit l);
    range (l + 1) u
  end
;;

let run (n : int) =
  let s = ref 0 in
  match
    range 0 n;
    !s
  with
  | v -> v
  | effect (Emit e), k ->
    s := !s + e;
    Effect.Deep.continue k ()
;;

let () =
  let n = try int_of_string (Array.get Sys.argv 1) with _ -> 40000000 in
  Printf.printf "%d\n" (run n)
