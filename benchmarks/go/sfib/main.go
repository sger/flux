package main

import "fmt"

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
	fmt.Println(sfib(20000000))
}
