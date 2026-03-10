import Control.Monad
import Control.Monad.ST
import Data.Array.IArray
import Data.Array.MArray
import Data.Array.ST
import Data.Word

type Elem = Word32

n :: Int
n = 32

badRand :: Elem -> Elem
badRand seed = seed * 1664525 + 1013904223

mkRandomArray :: Elem -> Int -> Array Int Elem
mkRandomArray seed size
  | size <= 0 = listArray (0, -1) []
  | otherwise = listArray (0, size - 1) $ take size $ iterate badRand seed

checkSortedChecksum :: Array Int Elem -> Int
checkSortedChecksum arr = go lo
  where
    (lo, hi) = bounds arr
    go i
      | hi < lo = 0
      | i < hi =
          if arr ! i <= arr ! (i + 1)
            then go (i + 1)
            else error "array is not sorted"
      | otherwise = fromIntegral (arr ! hi) + hi + 1

swap :: Int -> Int -> STArray s Int Elem -> ST s ()
swap i j arr = do
  x <- readArray arr i
  y <- readArray arr j
  writeArray arr i y
  writeArray arr j x

partitionAux :: STArray s Int Elem -> Int -> Elem -> Int -> Int -> ST s Int
partitionAux as hi pivot i j =
  if j < hi
    then do
      a <- readArray as j
      if a < pivot
        then do
          swap i j as
          partitionAux as hi pivot (i + 1) (j + 1)
        else partitionAux as hi pivot i (j + 1)
    else do
      swap i hi as
      pure i

partition :: STArray s Int Elem -> Int -> Int -> ST s Int
partition as lo hi = do
  let mid = (lo + hi) `div` 2
  amid <- readArray as mid
  alo <- readArray as lo
  when (amid < alo) (swap lo mid as)
  ahi <- readArray as hi
  alo' <- readArray as lo
  when (ahi < alo') (swap lo hi as)
  amid' <- readArray as mid
  ahi' <- readArray as hi
  when (amid' < ahi') (swap mid hi as)
  pivot <- readArray as hi
  partitionAux as hi pivot lo lo

qsortAux :: STArray s Int Elem -> Int -> Int -> ST s ()
qsortAux as low high =
  if low < high
    then do
      mid <- partition as low high
      qsortAux as low (mid - 1)
      qsortAux as (mid + 1) high
    else pure ()

qsort :: STArray s Int Elem -> ST s ()
qsort as = do
  (low, high) <- getBounds as
  qsortAux as low high

sortAndChecksum :: Int -> Int
sortAndChecksum i =
  let xs = mkRandomArray (fromIntegral i) i
      xs' =
        runSTArray $ do
          mxs <- thaw xs
          qsort mxs
          pure mxs
   in checkSortedChecksum xs'

benchInner :: Int -> Int -> Int -> Int
benchInner limit i acc =
  if i < limit
    then benchInner limit (i + 1) (acc + sortAndChecksum i)
    else acc

benchOuter :: Int -> Int -> Int -> Int
benchOuter limit iter acc =
  if iter < limit
    then benchOuter limit (iter + 1) (benchInner limit 0 acc)
    else acc

main :: IO ()
main = print (benchOuter n 0 0)
