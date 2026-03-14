data Color = Red | Black

data Tree a b
  = Leaf
  | Node !Color !(Tree a b) !a !b !(Tree a b)

limit :: Int
limit = 100000

foldTree :: (a -> b -> s -> s) -> Tree a b -> s -> s
foldTree _ Leaf b = b
foldTree f (Node _ l k v r) b = foldTree f r (f k v (foldTree f l b))

balance1 :: Tree a b -> Tree a b -> Tree a b
balance1 (Node _ _ kv vv t) (Node _ (Node Red l kx vx r1) ky vy r2) =
  Node Red (Node Black l kx vx r1) ky vy (Node Black r2 kv vv t)
balance1 (Node _ _ kv vv t) (Node _ l1 ky vy (Node Red l2 kx vx r)) =
  Node Red (Node Black l1 ky vy l2) kx vx (Node Black r kv vv t)
balance1 (Node _ _ kv vv t) (Node _ l ky vy r) =
  Node Black (Node Red l ky vy r) kv vv t
balance1 _ _ = Leaf

balance2 :: Tree a b -> Tree a b -> Tree a b
balance2 (Node _ t kv vv _) (Node _ (Node Red l kx1 vx1 r1) ky vy r2) =
  Node Red (Node Black t kv vv l) kx1 vx1 (Node Black r1 ky vy r2)
balance2 (Node _ t kv vv _) (Node _ l1 ky vy (Node Red l2 kx2 vx2 r2)) =
  Node Red (Node Black t kv vv l1) ky vy (Node Black l2 kx2 vx2 r2)
balance2 (Node _ t kv vv _) (Node _ l ky vy r) =
  Node Black t kv vv (Node Red l ky vy r)
balance2 _ _ = Leaf

isRed :: Tree a b -> Bool
isRed (Node Red _ _ _ _) = True
isRed _ = False

ins :: Tree Int Bool -> Int -> Bool -> Tree Int Bool
ins Leaf kx vx = Node Red Leaf kx vx Leaf
ins (Node Red a ky vy b) kx vx =
  if kx < ky
    then Node Red (ins a kx vx) ky vy b
    else if ky < kx
      then Node Red a ky vy (ins b kx vx)
      else Node Red a ky vy (ins b kx vx)
ins (Node Black a ky vy b) kx vx =
  if kx < ky
    then if isRed a
      then balance1 (Node Black Leaf ky vy b) (ins a kx vx)
      else Node Black (ins a kx vx) ky vy b
    else if ky < kx
      then if isRed b
        then balance2 (Node Black a ky vy Leaf) (ins b kx vx)
        else Node Black a ky vy (ins b kx vx)
      else Node Black a kx vx b

setBlack :: Tree a b -> Tree a b
setBlack (Node _ l k v r) = Node Black l k v r
setBlack e = e

insert :: Tree Int Bool -> Int -> Bool -> Tree Int Bool
insert t k v =
  if isRed t then setBlack (ins t k v) else ins t k v

mkMapAux :: Int -> Tree Int Bool -> Tree Int Bool
mkMapAux 0 m = m
mkMapAux n m =
  let m' = insert m n (n `mod` 10 == 0)
   in seq m' (mkMapAux (n - 1) m')

countTrue :: Tree Int Bool -> Int
countTrue tree = foldTree (\_ v acc -> if v then acc + 1 else acc) tree 0

main :: IO ()
main = print (countTrue (mkMapAux limit Leaf))
