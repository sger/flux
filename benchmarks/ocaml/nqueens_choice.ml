(* Port of benchmarks/flux/nqueens_choice_direct.flx. OCaml 5's
   continuations resume once, and `pick` resumes once per row, so this port
   tries every row in turn instead. *)
let rec safe queen diag = function
  | [] -> true
  | q :: qs -> queen <> q && queen <> q + diag && queen <> q - diag && safe queen (diag + 1) qs
;;

let rec place size column rest =
  if column > size then 1 else rows size column rest 1 0

and rows size column rest row a =
  if row > size then a
  else
    let found = if safe row 1 rest then place size (column + 1) (row :: rest) else 0 in
    rows size column rest (row + 1) (a + found)
;;

let () =
  let n = try int_of_string (Array.get Sys.argv 1) with _ -> 12 in
  Printf.printf "%d\n" (place n 1 [])
