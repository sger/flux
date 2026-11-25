// Port of benchmarks/flux/counter_direct.flx: Go has no effect handlers,
// so the state is the loop's counter, beside the step count.
package main

import (
	"fmt"
	"os"
	"strconv"
)

func counter(i int) int {
	c := 0
	for i != 0 {
		i--
		c++
	}
	return c
}

func main() {
	// The n is the last argument: under `yaegi run <path> <n>`,
	// os.Args still holds the interpreter's own "run" and path in front of it.
	n := 100100100
	if len(os.Args) > 1 {
		if v, err := strconv.Atoi(os.Args[len(os.Args)-1]); err == nil {
			n = v
		}
	}
	fmt.Println(counter(n))
}
