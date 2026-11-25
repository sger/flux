// Port of benchmarks/flux/resume_nontail_direct.flx: Go has no effect
// handlers, so the work after each resume is the work after a call.
package main

import (
	"fmt"
	"os"
	"strconv"
)

func combine(x, y int) int {
	v := x - 503*y + 37
	if v < 0 {
		v = -v
	}
	return v % 1009
}

func run(i, s int) int {
	if i == 0 {
		return s
	}
	return combine(i, run(i-1, s))
}

func main() {
	// The n is the last argument: under `yaegi run <path> <n>`,
	// os.Args still holds the interpreter's own "run" and path in front of it.
	n := 10000
	if len(os.Args) > 1 {
		if v, err := strconv.Atoi(os.Args[len(os.Args)-1]); err == nil {
			n = v
		}
	}
	s := 0
	for l := 0; l < 1000; l++ {
		s = run(n, s)
	}
	fmt.Println(s)
}
