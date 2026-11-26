// Port of benchmarks/flux/generator_direct.flx: Go has no effect
// handlers, so the tree is summed by a walk in order. Subtrees are shared
// pointers.
package main

import (
	"fmt"
	"os"
	"strconv"
)

type tree struct {
	left, right *tree
	value       int
}

func makeTree(n int) *tree {
	if n == 0 {
		return nil
	}
	t := makeTree(n - 1)
	return &tree{left: t, value: n, right: t}
}

func total(t *tree, a int) int {
	if t == nil {
		return a
	}
	return total(t.right, total(t.left, a)+t.value)
}

func main() {
	// The n is the last argument: under `yaegi run <path> <n>`,
	// os.Args still holds the interpreter's own "run" and path in front of it.
	n := 25
	if len(os.Args) > 1 {
		if v, err := strconv.Atoi(os.Args[len(os.Args)-1]); err == nil {
			n = v
		}
	}
	fmt.Println(total(makeTree(n), 0))
}
