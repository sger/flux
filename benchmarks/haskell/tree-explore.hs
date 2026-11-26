-- Port of benchmarks/flux/tree_explore_direct.flx: the state is threaded by
-- hand through the branches, and each choice expands into both branches'
-- results.
import System.Environment
import Data.List (foldl')

data Tree = Leaf | Node Tree !Int Tree

make :: Int -> Tree
make 0 = Leaf
make n = let t = make (n - 1) in Node t n t

operator :: Int -> Int -> Int
operator x y = abs (x - 503 * y + 37) `mod` 1009

explore :: Tree -> Int -> ([Int], Int)
explore Leaf s = ([s], s)
explore (Node l v r) s =
  let (xs, s2) = explore l (operator s v)
      (ys, s4) = s2 `seq` explore r (operator s2 v)
   in (map (operator v) xs ++ map (operator v) ys, s4)

turn :: Int -> Tree -> Int -> Int
turn 0 _ s = s
turn i t s = let (xs, _) = explore t s; m = foldl' max 0 xs in m `seq` turn (i - 1) t m

main = do
  args <- getArgs
  let n =
        case args of
          [] -> 16
          (arg : _) -> read arg
  print (turn 10 (make n) 0)
