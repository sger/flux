(* Port of benchmarks/flux/resume_nontail.flx with OCaml 5's effect
   handlers: the clause continues the computation, then works on its
   result. *)
type _ Effect.t += Operator : int -> unit Effect.t

let combine x y = abs (x - (503 * y) + 37) mod 1009

let rec loop i s =
  if i = 0 then s
  else begin
    Effect.perform (Operator i);
    loop (i - 1) s
  end
;;

let run n s =
  match loop n s with
  | v -> v
  | effect (Operator x), k -> combine x (Effect.Deep.continue k ())
;;

let rec repeat n l s = if l = 0 then s else repeat n (l - 1) (run n s)

let () =
  let n = try int_of_string (Array.get Sys.argv 1) with _ -> 10000 in
  Printf.printf "%d\n" (repeat n 1000 0)
