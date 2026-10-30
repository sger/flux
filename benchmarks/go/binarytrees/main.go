package main

import (
	"fmt"
	"os"
	"strconv"
)

// A nil *Tree is Tip.
type Tree struct {
	left  *Tree
	right *Tree
}

func maxInt(a, b int) int {
	if a > b {
		return a
	}
	return b
}

func pow2(exp, acc int) int {
	for exp > 0 {
		exp--
		acc *= 2
	}
	return acc
}

func makeTree(depth int) *Tree {
	if depth > 0 {
		return &Tree{makeTree(depth - 1), makeTree(depth - 1)}
	}
	return &Tree{nil, nil}
}

func checkTree(tree *Tree) int {
	if tree == nil {
		return 0
	}
	return checkTree(tree.left) + checkTree(tree.right) + 1
}

func sumTrees(iterations, depth int) int {
	acc := 0
	for ; iterations > 0; iterations-- {
		tree := makeTree(depth)
		acc += checkTree(tree)
	}
	return acc
}

func runDepths(depth, maxDepth, minDepth int) {
	for ; depth <= maxDepth; depth += 2 {
		iterations := pow2(maxDepth+minDepth-depth, 1)
		total := sumTrees(iterations, depth)
		fmt.Printf("%d\t trees of depth %d\t check: %d\n", iterations, depth, total)
	}
}

func main() {
	// The depth is the last argument: under `yaegi run <path> <depth>`,
	// os.Args still holds the interpreter's own "run" and path in front of it.
	n := 21
	if len(os.Args) > 1 {
		if v, err := strconv.Atoi(os.Args[len(os.Args)-1]); err == nil {
			n = v
		}
	}
	minDepth := 4
	maxDepth := maxInt(minDepth+2, n)
	stretchDepth := maxDepth + 1

	fmt.Printf("stretch tree of depth %d\t check: %d\n", stretchDepth, checkTree(makeTree(stretchDepth)))

	longLivedTree := makeTree(maxDepth)

	runDepths(minDepth, maxDepth, minDepth)

	fmt.Printf("long lived tree of depth %d\t check: %d\n", maxDepth, checkTree(longLivedTree))
}
