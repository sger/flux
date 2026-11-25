{-# LANGUAGE BangPatterns #-}

import Data.Int (Int32)
import System.Environment

-- Int32 wraps on overflow, as the sequence is defined.
sfibx :: Int -> Int32 -> Int32 -> Int32
sfibx !n !x1 !x2
  | n <= 0 = x1
  | x2 > 1000000000 = sfibx (n - 1) 1 2
  | otherwise = sfibx (n - 1) (x2 - x1) (x1 * x2 + x1)

main = do
  args <- getArgs
  let n =
        case args of
          [] -> 20000000
          (arg : _) -> read arg
  print (sfibx n 1 2)
