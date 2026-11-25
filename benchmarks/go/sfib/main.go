package main

import (
	"fmt"
	"os"
	"strconv"
)

func sfib(n int) int32 {
	var x1, x2 int32 = 1, 2
	for ; n > 0; n-- {
		if x2 > 1000000000 {
			x1, x2 = 1, 2
		} else {
			x1, x2 = x2-x1, x1*x2+x1
		}
	}
	return x1
}

func main() {
	// The n is the last argument: under `yaegi run <path> <n>`,
	// os.Args still holds the interpreter's own "run" and path in front of it.
	n := 20000000
	if len(os.Args) > 1 {
		if v, err := strconv.Atoi(os.Args[len(os.Args)-1]); err == nil {
			n = v
		}
	}
	fmt.Println(sfib(n))
}
