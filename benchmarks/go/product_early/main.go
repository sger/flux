// Port of benchmarks/flux/product_early_direct.flx: Go has no effect
// handlers, so the product returns 0 at the first zero.
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

func product(xs *list) int {
	if xs == nil {
		return 0
	}
	if xs.head == 0 {
		return 0
	}
	return xs.head * product(xs.tail)
}

func enumerate(i int) *list {
	var xs *list
	for v := 0; v <= i; v++ {
		xs = &list{v, xs}
	}
	return xs
}

func main() {
	// The n is the last argument: under `yaegi run <path> <n>`,
	// os.Args still holds the interpreter's own "run" and path in front of it.
	n := 100000
	if len(os.Args) > 1 {
		if v, err := strconv.Atoi(os.Args[len(os.Args)-1]); err == nil {
			n = v
		}
	}
	xs := enumerate(1000)
	a := 0
	for i := 0; i < n; i++ {
		a += product(xs)
	}
	fmt.Println(a)
}
