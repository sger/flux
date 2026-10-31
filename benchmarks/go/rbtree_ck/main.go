package main

import "fmt"

const (
	red   = 0
	black = 1
)

const limit = 100000

// A nil *Tree is Leaf. Nodes are never mutated: every insert copies the
// path it walks and shares the rest, as the Flux version does.
type Tree struct {
	color int
	left  *Tree
	key   int
	value bool
	right *Tree
}

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

func balance1(leftTree, rightTree *Tree) *Tree {
	if leftTree == nil || rightTree == nil {
		return nil
	}
	kv, vv, tail := leftTree.key, leftTree.value, leftTree.right
	l, ky, vy, r := rightTree.left, rightTree.key, rightTree.value, rightTree.right
	if isRed(l) {
		return node(red, node(black, l.left, l.key, l.value, l.right), ky, vy, node(black, r, kv, vv, tail))
	}
	if isRed(r) {
		return node(red, node(black, l, ky, vy, r.left), r.key, r.value, node(black, r.right, kv, vv, tail))
	}
	return node(black, node(red, l, ky, vy, r), kv, vv, tail)
}

func balance2(leftTree, rightTree *Tree) *Tree {
	if leftTree == nil || rightTree == nil {
		return nil
	}
	tree0, kv, vv := leftTree.left, leftTree.key, leftTree.value
	l, ky, vy, r := rightTree.left, rightTree.key, rightTree.value, rightTree.right
	if isRed(l) {
		return node(red, node(black, tree0, kv, vv, l.left), l.key, l.value, node(black, l.right, ky, vy, r))
	}
	if isRed(r) {
		return node(red, node(black, tree0, kv, vv, l), ky, vy, node(black, r.left, r.key, r.value, r.right))
	}
	return node(black, tree0, kv, vv, node(red, l, ky, vy, r))
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
			return balance1(node(black, nil, ky, vy, b), ins(a, kx, vx))
		}
		return node(black, ins(a, kx, vx), ky, vy, b)
	}
	if ky < kx {
		if isRed(b) {
			return balance2(node(black, a, ky, vy, nil), ins(b, kx, vx))
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

func mkMapAux(n int, tree *Tree) *Tree {
	for n > 0 {
		tree = insert(tree, n, n%10 == 0)
		n--
	}
	return tree
}

func main() {
	fmt.Println(countTrue(mkMapAux(limit, nil)))
}
