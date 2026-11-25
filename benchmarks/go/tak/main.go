package main

import (
	"fmt"
	"os"
	"strconv"
)

func tak(x, y, z int) int {
	if y < x {
		return tak(tak(x-1, y, z), tak(y-1, z, x), tak(z-1, x, y))
	}
	return z
}

func main() {
	// The n is the last argument: under `yaegi run <path> <n>`,
	// os.Args still holds the interpreter's own "run" and path in front of it.
	n := 30
	if len(os.Args) > 1 {
		if v, err := strconv.Atoi(os.Args[len(os.Args)-1]); err == nil {
			n = v
		}
	}
	fmt.Println(tak(n, n*2/3, n/3))
}
