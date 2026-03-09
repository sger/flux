--
-- The Computer Language Benchmarks Game
-- https://salsa.debian.org/benchmarksgame-team/benchmarksgame/
--
-- Contributed by Don Stewart
-- Basic parallelization by Roman Kashitsyn
-- Tail call optimizations by Izaak Weiss
--
{-# LANGUAGE BangPatterns #-}

import Data.Bits
import System.Environment
import Text.Printf

data Tree = Nil | Node !Tree !Tree

minN :: Int
minN = 4

io :: String -> Int -> Int -> IO ()
io s n t = printf "%s of depth %d\t check: %d\n" s n t

main :: IO ()
main = do
    args <- getArgs
    let n =
            case args of
                [] -> 21
                (arg : _) -> read arg
    let maxN = max (minN + 2) n
        stretchN = maxN + 1

    let c = check (make stretchN)
    io "stretch tree" stretchN c

    let !long = make maxN

    let vs = depth minN maxN
    mapM_ (\(m, d, i) -> io (show m ++ "\t trees") d i) vs

    io "long lived tree" maxN (check long)

depth :: Int -> Int -> [(Int, Int, Int)]
depth d m
    | d <= m = (n, d, sumT d n 0) : depth (d + 2) m
    | otherwise = []
  where
    n = 1 `shiftL` (m - d + minN)

sumT :: Int -> Int -> Int -> Int
sumT _ 0 t = t
sumT d i t = sumT d (i - 1) (t + a)
  where
    a = check (make d)

check :: Tree -> Int
check t = tailCheck t 0

tailCheck :: Tree -> Int -> Int
tailCheck Nil !a = a
tailCheck (Node l r) !a = tailCheck l $ tailCheck r $ a + 1

make :: Int -> Tree
make d = make' d d

make' :: Int -> Int -> Tree
make' _ 0 = Node Nil Nil
make' !n d = Node (make' (n - 1) (d - 1)) (make' (n + 1) (d - 1))
