(* OCaml ints wrap at 63 bits, so masking to 32 bits afterwards gives the
   same result as 32-bit arithmetic. *)
let wrap32 v =
  let m = v land 0xFFFFFFFF in
  if m >= 0x80000000 then m - 0x100000000 else m
;;

let rec sfibx n x1 x2 =
  if n <= 0 then x1
  else if x2 > 1000000000 then sfibx (n - 1) 1 2
  else sfibx (n - 1) (wrap32 (x2 - x1)) (wrap32 (x1 * x2 + x1))
;;

let () =
  let n = try int_of_string (Array.get Sys.argv 1) with _ -> 20000000 in
  Printf.printf "%d\n" (sfibx n 1 2)
