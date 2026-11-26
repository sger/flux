-- Port of benchmarks/flux/generator_direct.flx: the tree is summed by a
-- walk in order.
import System.Environment

data Tree = Leaf | Node Tree !Int Tree

make :: Int -> Tree
make 0 = Leaf
make n = let t = make (n - 1) in Node t n t

total :: Tree -> Int -> Int
total Leaf a = a
total (Node l v r) a = let b = total l a + v in b `seq` total r b

main = do
  args <- getArgs
  let n =
        case args of
          [] -> 25
          (arg : _) -> read arg
  print (total (make n) 0)
