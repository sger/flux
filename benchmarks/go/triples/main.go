// Port of benchmarks/flux/triples_direct.flx: Go has no effect handlers,
// so the triples i > j > k that sum to n are visited in nested loops and
// their hashes added as they are found.
package main

import (
	"fmt"
	"os"
	"strconv"
)

const m = 1000000007

func run(n int) int {
	acc := 0
	for i := n; i >= 1; i-- {
		for j := i - 1; j >= 1; j-- {
			for k := j - 1; k >= 1; k-- {
				if i+j+k == n {
					acc = (acc + (53*i+2809*j+148877*k)%m) % m
				}
			}
		}
	}
	return acc
}

func main() {
	// The n is the last argument: under `yaegi run <path> <n>`,
	// os.Args still holds the interpreter's own "run" and path in front of it.
	n := 10
	if len(os.Args) > 1 {
		if v, err := strconv.Atoi(os.Args[len(os.Args)-1]); err == nil {
			n = v
		}
	}
	fmt.Println(run(n))
}
