package main

import "fmt"

// A nil *Solution is Nil. Cells are never mutated, so every partial solution
// shares its tail with the one it was built from.
type Solution struct {
	q    int
	tail *Solution
}

func safe(queen, diag int, xs *Solution) bool {
	for ; xs != nil; xs = xs.tail {
		q := xs.q
		if queen == q || queen == q+diag || queen == q-diag {
			return false
		}
		diag++
	}
	return true
}

func place(n, row int, soln *Solution) int {
	if row == 0 {
		return 1
	}
	return tryCol(n, row, soln, n)
}

func tryCol(n, row int, soln *Solution, col int) int {
	if col <= 0 {
		return 0
	}
	if safe(col, 1, soln) {
		return place(n, row-1, &Solution{col, soln}) + tryCol(n, row, soln, col-1)
	}
	return tryCol(n, row, soln, col-1)
}

func queens(n int) int {
	return place(n, n, nil)
}

func main() {
	fmt.Println(queens(13))
}
