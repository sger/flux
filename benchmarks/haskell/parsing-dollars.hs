{-# LANGUAGE BangPatterns #-}
-- Port of benchmarks/flux/parsing_dollars_direct.flx: each char is made
-- from the two counters where it is read.
import System.Environment

feed :: Int -> Int -> Int -> Int -> Int -> Int
feed n !i !j !a !s
  | i > n = s
  | j == 0 = parse n (i + 1) (i + 1) 10 a s
  | otherwise = parse n i (j - 1) 36 a s

parse :: Int -> Int -> Int -> Int -> Int -> Int -> Int
parse n !i !j !c !a !s
  | c == 36 = feed n i j (a + 1) s
  | c == 10 = feed n i j 0 (s + a)
  | otherwise = s

main = do
  args <- getArgs
  let n =
        case args of
          [] -> 20000
          (arg : _) -> read arg
  print (feed n 0 0 0 0)
