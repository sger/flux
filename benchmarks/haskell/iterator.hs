-- Port of benchmarks/flux/iterator_direct.flx: each emitted value is
-- added where it is made.
import System.Environment

range' :: Int -> Int -> Int -> Int
range' l u s = if l > u then s else range' (l + 1) u (s + l)

main = do
  args <- getArgs
  let n =
        case args of
          [] -> 40000000
          (arg : _) -> read arg
  print (range' 0 n 0)
