// Port of benchmarks/flux/nqueens_choice_direct.flx: Go has no effect
// handlers, so every row is tried in a loop instead of by resuming `pick`.
package main

import (
	"fmt"
	"os"
	"strconv"
)

type list struct {
	head int
	tail *list
}

func safe(queen, diag int, xs *list) bool {
	for ; xs != nil; xs, diag = xs.tail, diag+1 {
		q := xs.head
		if queen == q || queen == q+diag || queen == q-diag {
			return false
		}
	}
	return true
}

func place(size, column int, rest *list) int {
	if column > size {
		return 1
	}
	a := 0
	for row := 1; row <= size; row++ {
		if safe(row, 1, rest) {
			a += place(size, column+1, &list{row, rest})
		}
	}
	return a
}

func main() {
	// The n is the last argument: under `yaegi run <path> <n>`,
	// os.Args still holds the interpreter's own "run" and path in front of it.
	n := 12
	if len(os.Args) > 1 {
		if v, err := strconv.Atoi(os.Args[len(os.Args)-1]); err == nil {
			n = v
		}
	}
	fmt.Println(place(n, 1, nil))
}
