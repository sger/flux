(* Port of benchmarks/flux/triples_direct.flx: OCaml 5's continuations are
   one-shot, and the flip is resumed twice, so the triples are visited in
   nested loops and their hashes added as they are found. *)
let m = 1000000007

let hash i j k = ((53 * i) + (2809 * j) + (148877 * k)) mod m

let run (n : int) =
  let acc = ref 0 in
  for i = n downto 1 do
    for j = i - 1 downto 1 do
      for k = j - 1 downto 1 do
        if i + j + k = n then acc := (!acc + hash i j k) mod m
      done
    done
  done;
  !acc
;;

let () =
  let n = try int_of_string (Array.get Sys.argv 1) with _ -> 10 in
  Printf.printf "%d\n" (run n)
