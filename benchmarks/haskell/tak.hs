import System.Environment

tak :: Int -> Int -> Int -> Int
tak x y z
  = if y < x
      then tak (tak (x - 1) y z) (tak (y - 1) z x) (tak (z - 1) x y)
      else z

main = do
  args <- getArgs
  let n =
        case args of
          [] -> 30
          (arg : _) -> read arg
  print (tak n (n * 2 `div` 3) (n `div` 3))
