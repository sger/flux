-- Port of benchmarks/flux/resume_nontail_direct.flx: the work after each
-- resume is the work after a call.
import System.Environment

combine :: Int -> Int -> Int
combine x y = abs (x - 503 * y + 37) `mod` 1009

run :: Int -> Int -> Int
run i s = if i == 0 then s else combine i (run (i - 1) s)

repeat' :: Int -> Int -> Int -> Int
repeat' n l s = if l == 0 then s else repeat' n (l - 1) (run n s)

main = do
  args <- getArgs
  let n =
        case args of
          [] -> 10000
          (arg : _) -> read arg
  print (repeat' n 1000 0)
