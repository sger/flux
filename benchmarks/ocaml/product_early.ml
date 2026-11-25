(* Port of benchmarks/flux/product_early.flx with OCaml 5's effect
   handlers: Done aborts the product at the first zero, never resumed. *)
type _ Effect.t += Done : int -> int Effect.t

let rec product = function
  | [] -> 0
  | y :: ys -> if y = 0 then Effect.perform (Done 0) else y * product ys
;;

let run_product xs = match product xs with v -> v | effect (Done r), _ -> r

let rec enumerate i = if i < 0 then [] else i :: enumerate (i - 1)

let rec run xs i a = if i = 0 then a else run xs (i - 1) (a + run_product xs)

let () =
  let n = try int_of_string (Array.get Sys.argv 1) with _ -> 100000 in
  Printf.printf "%d\n" (run (enumerate 1000) n 0)
