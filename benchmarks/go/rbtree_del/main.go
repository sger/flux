package main

import "fmt"

const (
	red   = 0
	black = 1
)

const limit = 100000

// A nil *Tree is Leaf. Nodes are never mutated: insert and delete copy the
// path they walk and share the rest, as the Flux version does.
type Tree struct {
	color int
	left  *Tree
	key   int
	value bool
	right *Tree
}

// Flux's Del(tree, shrunk) and Delmin(Del, key, value) are returned as
// multiple results instead of allocated wrapper values.

func node(c int, l *Tree, k int, v bool, r *Tree) *Tree {
	return &Tree{c, l, k, v, r}
}

func countTrue(tree *Tree) int {
	if tree == nil {
		return 0
	}
	c := countTrue(tree.left) + countTrue(tree.right)
	if tree.value {
		c++
	}
	return c
}

func isRed(tree *Tree) bool {
	return tree != nil && tree.color == red
}

func isBlackColor(color int) bool {
	return color == black
}

func balanceLeft(l *Tree, k int, v bool, r *Tree) *Tree {
	if l == nil {
		return nil
	}
	if isRed(l.left) {
		x := l.left
		return node(red, node(black, x.left, x.key, x.value, x.right), l.key, l.value, node(black, l.right, k, v, r))
	}
	if isRed(l.right) {
		x := l.right
		return node(red, node(black, l.left, l.key, l.value, x.left), x.key, x.value, node(black, x.right, k, v, r))
	}
	return node(black, node(red, l.left, l.key, l.value, l.right), k, v, r)
}

func balanceRight(l *Tree, k int, v bool, r *Tree) *Tree {
	if r == nil {
		return nil
	}
	if isRed(r.left) {
		x := r.left
		return node(red, node(black, l, k, v, x.left), x.key, x.value, node(black, x.right, r.key, r.value, r.right))
	}
	if isRed(r.right) {
		y := r.right
		return node(red, node(black, l, k, v, r.left), r.key, r.value, node(black, y.left, y.key, y.value, y.right))
	}
	return node(black, l, k, v, node(red, r.left, r.key, r.value, r.right))
}

func ins(tree *Tree, kx int, vx bool) *Tree {
	if tree == nil {
		return node(red, nil, kx, vx, nil)
	}
	a, ky, vy, b := tree.left, tree.key, tree.value, tree.right
	if tree.color == red {
		if kx < ky {
			return node(red, ins(a, kx, vx), ky, vy, b)
		}
		if ky < kx {
			return node(red, a, ky, vy, ins(b, kx, vx))
		}
		return node(red, a, ky, vy, ins(b, kx, vx))
	}
	if kx < ky {
		if isRed(a) {
			return balanceLeft(ins(a, kx, vx), ky, vy, b)
		}
		return node(black, ins(a, kx, vx), ky, vy, b)
	}
	if ky < kx {
		if isRed(b) {
			return balanceRight(a, ky, vy, ins(b, kx, vx))
		}
		return node(black, a, ky, vy, ins(b, kx, vx))
	}
	return node(black, a, kx, vx, b)
}

func setBlack(tree *Tree) *Tree {
	if tree == nil {
		return nil
	}
	return node(black, tree.left, tree.key, tree.value, tree.right)
}

func insert(tree *Tree, key int, value bool) *Tree {
	if isRed(tree) {
		return setBlack(ins(tree, key, value))
	}
	return ins(tree, key, value)
}

func setRed(tree *Tree) *Tree {
	if tree == nil {
		return nil
	}
	return node(red, tree.left, tree.key, tree.value, tree.right)
}

func makeBlack(tree *Tree) (*Tree, bool) {
	if isRed(tree) {
		return node(black, tree.left, tree.key, tree.value, tree.right), false
	}
	return tree, true
}

func rebalanceLeft(c int, l *Tree, k int, v bool, r *Tree) (*Tree, bool) {
	if l == nil {
		return nil, false
	}
	if l.color == black {
		return balanceLeft(setRed(l), k, v, r), isBlackColor(c)
	}
	return node(black, l.left, l.key, l.value, balanceLeft(setRed(l.right), k, v, r)), false
}

func rebalanceRight(c int, l *Tree, k int, v bool, r *Tree) (*Tree, bool) {
	if r == nil {
		return nil, false
	}
	if r.color == black {
		return balanceRight(l, k, v, setRed(r)), isBlackColor(c)
	}
	return node(black, balanceRight(l, k, v, setRed(r.left)), r.key, r.value, r.right), false
}

func delMin(tree *Tree) (*Tree, bool, int, bool) {
	if tree == nil {
		return nil, false, 0, false
	}
	color, left, key, value, right := tree.color, tree.left, tree.key, tree.value, tree.right
	if left == nil {
		if color == black {
			if right == nil {
				return nil, true, key, value
			}
			return setBlack(right), false, key, value
		}
		return right, false, key, value
	}
	lx, shrunk, kx, vx := delMin(left)
	if shrunk {
		t, s := rebalanceRight(color, lx, key, value, right)
		return t, s, kx, vx
	}
	return node(color, lx, key, value, right), false, kx, vx
}

func del(tree *Tree, key int) (*Tree, bool) {
	if tree == nil {
		return nil, false
	}
	color, left, kx, vx, right := tree.color, tree.left, tree.key, tree.value, tree.right
	if key < kx {
		ly, shrunk := del(left, key)
		if shrunk {
			return rebalanceRight(color, ly, kx, vx, right)
		}
		return node(color, ly, kx, vx, right), false
	}
	if key > kx {
		ry, shrunk := del(right, key)
		if shrunk {
			return rebalanceLeft(color, left, kx, vx, ry)
		}
		return node(color, left, kx, vx, ry), false
	}
	if right == nil {
		if isBlackColor(color) {
			return makeBlack(left)
		}
		return left, false
	}
	ry, shrunk, ky, vy := delMin(right)
	if shrunk {
		return rebalanceLeft(color, left, ky, vy, ry)
	}
	return node(color, left, ky, vy, ry), false
}

func deleteKey(tree *Tree, key int) *Tree {
	next, _ := del(tree, key)
	return setBlack(next)
}

func mkMapAux(total, n int, tree *Tree) *Tree {
	for n > 0 {
		n1 := n - 1
		t1 := insert(tree, n1, n1%10 == 0)
		if n1%4 == 0 {
			tree = deleteKey(t1, n1+(total-n1)/5)
		} else {
			tree = t1
		}
		n = n1
	}
	return tree
}

func main() {
	fmt.Println(countTrue(mkMapAux(limit, limit, nil)))
}
