// Port of benchmarks/flux/tree_explore_direct.flx: Go has no effect
// handlers, so the state is threaded by hand through the branches and each
// choice expands into both branches' results. Subtrees are shared pointers.
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

func operator(x, y int) int {
	d := x - 503*y + 37
	if d < 0 {
		d = -d
	}
	return d % 1009
}

func explore(t *tree, s int) ([]int, int) {
	if t == nil {
		return []int{s}, s
	}
	xs, s2 := explore(t.left, operator(s, t.value))
	ys, s4 := explore(t.right, operator(s2, t.value))
	out := make([]int, 0, len(xs)+len(ys))
	for _, x := range xs {
		out = append(out, operator(t.value, x))
	}
	for _, y := range ys {
		out = append(out, operator(t.value, y))
	}
	return out, s4
}

func main() {
	// The n is the last argument: under `yaegi run <path> <n>`,
	// os.Args still holds the interpreter's own "run" and path in front of it.
	n := 16
	if len(os.Args) > 1 {
		if v, err := strconv.Atoi(os.Args[len(os.Args)-1]); err == nil {
			n = v
		}
	}
	t := makeTree(n)
	state := 0
	for i := 0; i < 10; i++ {
		xs, _ := explore(t, state)
		m := 0
		for _, x := range xs {
			if x > m {
				m = x
			}
		}
		state = m
	}
	fmt.Println(state)
}
