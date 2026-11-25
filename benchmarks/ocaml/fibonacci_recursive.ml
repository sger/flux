(* Port of benchmarks/flux/fibonacci_recursive.flx. *)
let rec fibonacci n = if n = 0 then 0 else if n = 1 then 1 else fibonacci (n - 1) + fibonacci (n - 2)

let () =
  let n = try int_of_string (Array.get Sys.argv 1) with _ -> 42 in
  Printf.printf "%d\n" (fibonacci n)
