-- Port of benchmarks/flux/product_early_direct.flx: the product returns 0
-- at the first zero.
import System.Environment

product' :: [Int] -> Int
product' [] = 0
product' (y : ys) = if y == 0 then 0 else y * product' ys

run :: [Int] -> Int -> Int -> Int
run xs i a = if i == 0 then a else run xs (i - 1) (a + product' xs)

main = do
  args <- getArgs
  let n =
        case args of
          [] -> 100000
          (arg : _) -> read arg
  print (run [1000, 999 .. 0] n 0)
