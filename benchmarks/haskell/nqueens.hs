data Solution = Nil | Cons !Int !Solution

safe queen diag xs
  = case xs of
      Nil -> True
      Cons q t -> queen /= q && queen /= q + diag && queen /= q - diag && safe queen (diag + 1) t

place n 0 soln
  = 1
place n row soln
  = try n
  where
    try col
      = if col <= 0
         then 0
         else if safe col 1 soln
              then place n (row - 1) (Cons col soln) + try (col - 1)
              else try (col - 1)

queens n
  = place n n Nil

main
  = print (queens 13)
