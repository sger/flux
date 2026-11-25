let rec tak x y z =
  if y < x then tak (tak (x - 1) y z) (tak (y - 1) z x) (tak (z - 1) x y)
  else z
;;

let () =
  let n = try int_of_string (Array.get Sys.argv 1) with _ -> 30 in
  Printf.printf "%d\n" (tak n (n * 2 / 3) (n / 3))
