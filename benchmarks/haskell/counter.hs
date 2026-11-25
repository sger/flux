-- Port of benchmarks/flux/counter_direct.flx: the state is the loop's
-- counter, beside the step count.
import System.Environment

counter :: Int -> Int -> Int
counter i c = if i == 0 then c else counter (i - 1) (c + 1)

main = do
  args <- getArgs
  let n =
        case args of
          [] -> 100100100
          (arg : _) -> read arg
  print (counter n 0)
