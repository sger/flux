(* Port of benchmarks/flux/parsing_dollars.flx with OCaml 5's effect
   handlers, as effect-handlers-bench's own OCaml version writes it: read
   and emit are effects whose clauses resume as their last act, and stop is
   an exception, which a clause may raise. *)
open Effect
open Effect.Deep

type _ Effect.t += Read : unit -> int t
type _ Effect.t += Emit : int -> unit t

exception Stop

let rec parse a =
  let c = perform (Read ()) in
  if c = 36 then parse (a + 1)
  else if c = 10 then begin
    perform (Emit a);
    parse 0
  end
  else raise Stop
;;

let sum action =
  let s = ref 0 in
  try
    action ();
    !s
  with
  | effect (Emit e), k ->
    s := !s + e;
    continue k ()
;;

let catch action = try action () with Stop -> ()

let feed n action =
  let i = ref 0 in
  let j = ref 0 in
  try action () with
  | effect (Read ()), k ->
    if !i > n then raise Stop
    else if !j = 0 then begin
      i := !i + 1;
      j := !i;
      continue k 10
    end
    else begin
      j := !j - 1;
      continue k 36
    end
;;

let run n = sum (fun () -> catch (fun () -> feed n (fun () -> parse 0)))

let () =
  let n = try int_of_string (Array.get Sys.argv 1) with _ -> 20000 in
  Printf.printf "%d\n" (run n)
