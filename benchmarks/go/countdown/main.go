// Port of benchmarks/flux/countdown_direct.flx: Go has no effect
// handlers, so the state is the loop's counter.
package main

import (
	"fmt"
	"os"
	"strconv"
)

func countdown(i int) int {
	for i != 0 {
		i--
	}
	return i
}

func main() {
	// The n is the last argument: under `yaegi run <path> <n>`,
	// os.Args still holds the interpreter's own "run" and path in front of it.
	n := 200000000
	if len(os.Args) > 1 {
		if v, err := strconv.Atoi(os.Args[len(os.Args)-1]); err == nil {
			n = v
		}
	}
	fmt.Println(countdown(n))
}
