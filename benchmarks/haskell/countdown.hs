-- Port of benchmarks/flux/countdown_direct.flx: the state is the loop's
-- counter.
import System.Environment

countdown :: Int -> Int
countdown i = if i == 0 then i else countdown (i - 1)

main = do
  args <- getArgs
  let n =
        case args of
          [] -> 200000000
          (arg : _) -> read arg
  print (countdown n)
