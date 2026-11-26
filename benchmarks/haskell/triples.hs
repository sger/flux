-- Port of benchmarks/flux/triples_direct.flx: the triples i > j > k that
-- sum to n, their hashes added as they are found.
import System.Environment

m :: Int
m = 1000000007

hash :: Int -> Int -> Int -> Int
hash i j k = (53 * i + 2809 * j + 148877 * k) `mod` m

ks :: Int -> Int -> Int -> Int -> Int -> Int
ks i j k s acc
  | k < 1 = acc
  | otherwise =
      let acc' = if i + j + k == s then (acc + hash i j k) `mod` m else acc
       in acc' `seq` ks i j (k - 1) s acc'

js :: Int -> Int -> Int -> Int -> Int
js i j s acc = if j < 1 then acc else let a = ks i j (j - 1) s acc in a `seq` js i (j - 1) s a

is :: Int -> Int -> Int -> Int
is i s acc = if i < 1 then acc else let a = js i (i - 1) s acc in a `seq` is (i - 1) s a

main = do
  args <- getArgs
  let n =
        case args of
          [] -> 10
          (arg : _) -> read arg
  print (is n n 0)
