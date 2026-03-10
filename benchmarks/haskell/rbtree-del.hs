data Color = Red | Black

data Tree a b
  = Leaf
  | Node !Color !(Tree a b) !a !b !(Tree a b)

limit :: Int
limit = 100000

foldTree :: (a -> b -> s -> s) -> Tree a b -> s -> s
foldTree _ Leaf b = b
foldTree f (Node _ l k v r) b = foldTree f r (f k v (foldTree f l b))

balanceLeft :: Tree a b -> a -> b -> Tree a b -> Tree a b
balanceLeft l k v r =
  case l of
    Leaf -> Leaf
    Node _ (Node Red lx kx vx rx) ky vy ry ->
      Node Red (Node Black lx kx vx rx) ky vy (Node Black ry k v r)
    Node _ ly ky vy (Node Red lx kx vx rx) ->
      Node Red (Node Black ly ky vy lx) kx vx (Node Black rx k v r)
    Node _ lx kx vx rx ->
      Node Black (Node Red lx kx vx rx) k v r

balanceRight :: Tree a b -> a -> b -> Tree a b -> Tree a b
balanceRight l k v r =
  case r of
    Leaf -> Leaf
    Node _ (Node Red lx kx vx rx) ky vy ry ->
      Node Red (Node Black l k v lx) kx vx (Node Black rx ky vy ry)
    Node _ lx kx vx (Node Red ly ky vy ry) ->
      Node Red (Node Black l k v lx) kx vx (Node Black ly ky vy ry)
    Node _ lx kx vx rx ->
      Node Black l k v (Node Red lx kx vx rx)

isRed :: Tree a b -> Bool
isRed (Node Red _ _ _ _) = True
isRed _ = False

isBlackColor :: Color -> Bool
isBlackColor Black = True
isBlackColor _ = False

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
      then balanceLeft (ins a kx vx) ky vy b
      else Node Black (ins a kx vx) ky vy b
    else if ky < kx
      then if isRed b
        then balanceRight a ky vy (ins b kx vx)
        else Node Black a ky vy (ins b kx vx)
      else Node Black a kx vx b

setBlack :: Tree a b -> Tree a b
setBlack (Node _ l k v r) = Node Black l k v r
setBlack e = e

insert :: Tree Int Bool -> Int -> Bool -> Tree Int Bool
insert t k v =
  if isRed t then setBlack (ins t k v) else ins t k v

data Del a b = Del (Tree a b) Bool

setRed :: Tree a b -> Tree a b
setRed t =
  case t of
    Node _ l k v r -> Node Red l k v r
    _ -> t

makeBlack :: Tree a b -> Del a b
makeBlack t =
  case t of
    Node Red l k v r -> Del (Node Black l k v r) False
    _ -> Del t True

rebalanceLeft :: Color -> Tree Int Bool -> Int -> Bool -> Tree Int Bool -> Del Int Bool
rebalanceLeft c l k v r =
  case l of
    Node Black _ _ _ _ -> Del (balanceLeft (setRed l) k v r) (isBlackColor c)
    Node Red lx kx vx rx -> Del (Node Black lx kx vx (balanceLeft (setRed rx) k v r)) False
    _ -> Del Leaf False

rebalanceRight :: Color -> Tree Int Bool -> Int -> Bool -> Tree Int Bool -> Del Int Bool
rebalanceRight c l k v r =
  case r of
    Node Black _ _ _ _ -> Del (balanceRight l k v (setRed r)) (isBlackColor c)
    Node Red lx kx vx rx -> Del (Node Black (balanceRight l k v (setRed lx)) kx vx rx) False
    _ -> Del Leaf False

data Delmin a b = Delmin (Del a b) a b

delMin :: Tree Int Bool -> Delmin Int Bool
delMin t =
  case t of
    Node Black Leaf k v r ->
      case r of
        Leaf -> Delmin (Del Leaf True) k v
        _ -> Delmin (Del (setBlack r) False) k v
    Node Red Leaf k v r -> Delmin (Del r False) k v
    Node c l k v r ->
      case delMin l of
        Delmin (Del lx True) kx vx -> Delmin (rebalanceRight c lx k v r) kx vx
        Delmin (Del lx False) kx vx -> Delmin (Del (Node c lx k v r) False) kx vx
    Leaf -> Delmin (Del Leaf False) 0 False

del :: Tree Int Bool -> Int -> Del Int Bool
del t k =
  case t of
    Leaf -> Del Leaf False
    Node cx lx kx vx rx ->
      if k < kx
        then case del lx k of
          Del ly True -> rebalanceRight cx ly kx vx rx
          Del ly False -> Del (Node cx ly kx vx rx) False
        else if k > kx
          then case del rx k of
            Del ry True -> rebalanceLeft cx lx kx vx ry
            Del ry False -> Del (Node cx lx kx vx ry) False
          else case rx of
            Leaf -> if isBlackColor cx then makeBlack lx else Del lx False
            _ ->
              case delMin rx of
                Delmin (Del ry True) ky vy -> rebalanceLeft cx lx ky vy ry
                Delmin (Del ry False) ky vy -> Del (Node cx lx ky vy ry) False

delete :: Tree Int Bool -> Int -> Tree Int Bool
delete t k =
  case del t k of
    Del tx _ -> setBlack tx

mkMapAux :: Int -> Int -> Tree Int Bool -> Tree Int Bool
mkMapAux total 0 t = t
mkMapAux total n t =
  let n1 = n - 1
      t1 = insert t n1 (n1 `mod` 10 == 0)
      t2 = if n1 `mod` 4 == 0 then delete t1 (n1 + (total - n1) `div` 5) else t1
   in seq t2 (mkMapAux total n1 t2)

countTrue :: Tree Int Bool -> Int
countTrue tree = foldTree (\_ v acc -> if v then acc + 1 else acc) tree 0

main :: IO ()
main = print (countTrue (mkMapAux limit limit Leaf))
