package main

import "fmt"

const (
	kVar = iota
	kVal
	kAdd
	kMul
)

// Expr is a tagged node: Var and Val use n, Add and Mul use left and right.
// Nodes are never mutated, so subtrees can be shared.
type Expr struct {
	kind  int
	n     int
	left  *Expr
	right *Expr
}

func mkVar(n int) *Expr      { return &Expr{kind: kVar, n: n} }
func mkVal(n int) *Expr      { return &Expr{kind: kVal, n: n} }
func mkAdd(l, r *Expr) *Expr { return &Expr{kind: kAdd, left: l, right: r} }
func mkMul(l, r *Expr) *Expr { return &Expr{kind: kMul, left: l, right: r} }
func isVal(e *Expr) bool     { return e.kind == kVal }

func dec(n int) int {
	if n == 0 {
		return 0
	}
	return n - 1
}

func mkExpr(n, v int) *Expr {
	if n == 0 {
		if v == 0 {
			return mkVar(1)
		}
		return mkVal(v)
	}
	return mkAdd(mkExpr(n-1, v+1), mkExpr(n-1, dec(v)))
}

func appendAdd(expr, tail *Expr) *Expr {
	if expr.kind == kAdd {
		return mkAdd(expr.left, appendAdd(expr.right, tail))
	}
	return mkAdd(expr, tail)
}

func appendMul(expr, tail *Expr) *Expr {
	if expr.kind == kMul {
		return mkMul(expr.left, appendMul(expr.right, tail))
	}
	return mkMul(expr, tail)
}

func reassoc(expr *Expr) *Expr {
	switch expr.kind {
	case kAdd:
		return appendAdd(reassoc(expr.left), reassoc(expr.right))
	case kMul:
		return appendMul(reassoc(expr.left), reassoc(expr.right))
	}
	return expr
}

func foldAdd(left, right *Expr) *Expr {
	if isVal(left) {
		a := left.n
		if isVal(right) {
			return mkVal(a + right.n)
		}
		if right.kind == kAdd {
			if isVal(right.right) {
				return mkAdd(mkVal(a+right.right.n), right.left)
			}
			if isVal(right.left) {
				return mkAdd(mkVal(a+right.left.n), right.right)
			}
		}
		return mkAdd(mkVal(a), right)
	}
	return mkAdd(left, right)
}

func foldMul(left, right *Expr) *Expr {
	if isVal(left) {
		a := left.n
		if isVal(right) {
			return mkVal(a * right.n)
		}
		if right.kind == kMul {
			if isVal(right.right) {
				return mkMul(mkVal(a*right.right.n), right.left)
			}
			if isVal(right.left) {
				return mkMul(mkVal(a*right.left.n), right.right)
			}
		}
		return mkMul(mkVal(a), right)
	}
	return mkMul(left, right)
}

func constFolding(expr *Expr) *Expr {
	switch expr.kind {
	case kAdd:
		return foldAdd(constFolding(expr.left), constFolding(expr.right))
	case kMul:
		return foldMul(constFolding(expr.left), constFolding(expr.right))
	}
	return expr
}

func eval(expr *Expr) int {
	switch expr.kind {
	case kVar:
		return 0
	case kVal:
		return expr.n
	case kAdd:
		return eval(expr.left) + eval(expr.right)
	}
	return eval(expr.left) * eval(expr.right)
}

func main() {
	original := mkExpr(12, 1)
	optimized := constFolding(reassoc(mkExpr(12, 1)))
	fmt.Printf("%d %d\n", eval(original), eval(optimized))
}
