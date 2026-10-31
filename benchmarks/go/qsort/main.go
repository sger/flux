package main

import "fmt"

const n = 32
const randMod = 4294967296

func badRand(seed int) int {
	return (seed*1664525 + 1013904223) % randMod
}

// concat returns a fresh slice and leaves both inputs untouched, matching
// Flow.Array.concat on persistent arrays.
func concat(a, b []int) []int {
	out := make([]int, len(a)+len(b))
	copy(out, a)
	copy(out[len(a):], b)
	return out
}

func mkRandomArray(seed, size int, acc []int) []int {
	for ; size > 0; size-- {
		acc = concat(acc, []int{seed})
		seed = badRand(seed)
	}
	return acc
}

func collectLt(xs []int, pivot, idx int, acc []int) []int {
	for ; idx < len(xs); idx++ {
		if x := xs[idx]; x < pivot {
			acc = concat(acc, []int{x})
		}
	}
	return acc
}

func collectGe(xs []int, pivot, idx int, acc []int) []int {
	for ; idx < len(xs); idx++ {
		if x := xs[idx]; x >= pivot {
			acc = concat(acc, []int{x})
		}
	}
	return acc
}

func qsort(xs []int) []int {
	if len(xs) <= 1 {
		return xs
	}
	pivot := xs[0]
	left := collectLt(xs, pivot, 1, []int{})
	right := collectGe(xs, pivot, 1, []int{})
	return concat(qsort(left), concat([]int{pivot}, qsort(right)))
}

func checkSortedChecksum(arr []int, i, hi int) int {
	if hi < 0 {
		return 0
	}
	for ; i < hi; i++ {
		if arr[i] > arr[i+1] {
			return -1
		}
	}
	return arr[hi] + hi + 1
}

func sortAndChecksum(i int) int {
	arr := mkRandomArray(i, i, []int{})
	sorted := qsort(arr)
	return checkSortedChecksum(sorted, 0, len(sorted)-1)
}

func main() {
	acc := 0
	for iter := 0; iter < n; iter++ {
		for i := 0; i < n; i++ {
			acc += sortAndChecksum(i)
		}
	}
	fmt.Println(acc)
}
