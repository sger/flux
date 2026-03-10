type solution =
| Nil
| Cons of int * solution
;;

let rec safe queen diag xs =
  match xs with
  | Nil -> true
  | Cons (q, tail) ->
    queen <> q &&
    queen <> q + diag &&
    queen <> q - diag &&
    safe queen (diag + 1) tail
;;

let rec place n row soln =
  if row = 0 then 1
  else try_col n row soln n

and try_col n row soln col =
  if col <= 0 then 0
  else if safe col 1 soln then place n (row - 1) (Cons (col, soln)) + try_col n row soln (col - 1)
  else try_col n row soln (col - 1)
;;

let queens n =
  place n n Nil
;;

let () =
  Printf.printf "%d\n" (queens 13)
