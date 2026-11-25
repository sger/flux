-- Port of benchmarks/flux/nqueens_choice_direct.flx: every row is tried in
-- turn instead of by resuming `pick`.
import System.Environment

safe :: Int -> Int -> [Int] -> Bool
safe _ _ [] = True
safe queen diag (q : qs) =
  queen /= q && queen /= q + diag && queen /= q - diag && safe queen (diag + 1) qs

place :: Int -> Int -> [Int] -> Int
place size column rest
  | column > size = 1
  | otherwise = rows 1 0
  where
    rows row a
      | row > size = a
      | safe row 1 rest = rows (row + 1) (a + place size (column + 1) (row : rest))
      | otherwise = rows (row + 1) a

main = do
  args <- getArgs
  let n =
        case args of
          [] -> 12
          (arg : _) -> read arg
  print (place n 1 [])
