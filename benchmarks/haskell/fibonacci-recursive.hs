-- Port of benchmarks/flux/fibonacci_recursive.flx.
import System.Environment

fibonacci :: Int -> Int
fibonacci 0 = 0
fibonacci 1 = 1
fibonacci n = fibonacci (n - 1) + fibonacci (n - 2)

main = do
  args <- getArgs
  let n =
        case args of
          [] -> 42
          (arg : _) -> read arg
  print (fibonacci n)
