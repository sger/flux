// Port of benchmarks/flux/iterator_direct.flx: Go has no effect
// handlers, so each emitted value is added where it is made.
package main

import (
	"fmt"
	"os"
	"strconv"
)

func rangeSum(l, u int) int {
	s := 0
	for i := l; i <= u; i++ {
		s += i
	}
	return s
}

func main() {
	// The n is the last argument: under `yaegi run <path> <n>`,
	// os.Args still holds the interpreter's own "run" and path in front of it.
	n := 40000000
	if len(os.Args) > 1 {
		if v, err := strconv.Atoi(os.Args[len(os.Args)-1]); err == nil {
			n = v
		}
	}
	fmt.Println(rangeSum(0, n))
}
