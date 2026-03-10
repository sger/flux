type elem = int

let n = 32
let mask = 0xFFFF_FFFF

let bad_rand seed =
  Int.logand ((seed * 1664525) + 1013904223) mask
;;

let mk_random_array seed n =
  let xs = Array.make n 0 in
  let cur = ref seed in
  for i = 0 to n - 1 do
    xs.(i) <- !cur;
    cur := bad_rand !cur
  done;
  xs
;;

let check_sorted_checksum xs =
  let len = Array.length xs in
  if len = 0 then 0
  else begin
    for i = 0 to len - 2 do
      if xs.(i) > xs.(i + 1) then failwith "array is not sorted"
    done;
    xs.(len - 1) + len
  end
;;

let swap xs i j =
  let tmp = xs.(i) in
  xs.(i) <- xs.(j);
  xs.(j) <- tmp
;;

let partition xs lo hi =
  let mid = (lo + hi) / 2 in
  if xs.(mid) < xs.(lo) then swap xs lo mid;
  if xs.(hi) < xs.(lo) then swap xs lo hi;
  if xs.(mid) < xs.(hi) then swap xs mid hi;
  let pivot = xs.(hi) in
  let i = ref lo in
  for j = lo to hi - 1 do
    if xs.(j) < pivot then begin
      swap xs !i j;
      incr i
    end
  done;
  swap xs !i hi;
  !i
;;

let rec qsort_aux xs lo hi =
  if lo < hi then begin
    let mid = partition xs lo hi in
    qsort_aux xs lo (mid - 1);
    qsort_aux xs (mid + 1) hi
  end
;;

let qsort xs =
  let len = Array.length xs in
  if len > 0 then qsort_aux xs 0 (len - 1)
;;

let sort_and_checksum i =
  let xs = mk_random_array i i in
  qsort xs;
  check_sorted_checksum xs
;;

let bench () =
  let acc = ref 0 in
  for _ = 0 to n - 1 do
    for i = 0 to n - 1 do
      acc := !acc + sort_and_checksum i
    done
  done;
  !acc
;;

let () =
  Printf.printf "%d\n" (bench ())
