// Port of benchmarks/flux/parsing_dollars_direct.flx: Go has no effect
// handlers, so each char is made from the two counters where it is read,
// and the two mutually recursive functions are one loop.
package main

import (
	"fmt"
	"os"
	"strconv"
)

func run(n int) int {
	i, j, a, s := 0, 0, 0, 0
	for {
		if i > n {
			return s
		}
		var c int
		if j == 0 {
			i++
			j = i
			c = 10
		} else {
			j--
			c = 36
		}
		if c == 36 {
			a++
		} else if c == 10 {
			s += a
			a = 0
		} else {
			return s
		}
	}
}

func main() {
	// The n is the last argument: under `yaegi run <path> <n>`,
	// os.Args still holds the interpreter's own "run" and path in front of it.
	n := 20000
	if len(os.Args) > 1 {
		if v, err := strconv.Atoi(os.Args[len(os.Args)-1]); err == nil {
			n = v
		}
	}
	fmt.Println(run(n))
}
