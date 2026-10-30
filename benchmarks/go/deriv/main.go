package main

import "fmt"

const (
	kVal = iota
	kVar
	kAdd
	kMul
	kPow
	kLn
)

// Expr is a tagged node: Val and Var use n, Ln uses left, the binary forms
// use left and right. Nodes are never mutated, so subtrees are shared.
type Expr struct {
	kind  int
	n     int
	left  *Expr
	right *Expr
}

func val(n int) *Expr            { return &Expr{kind: kVal, n: n} }
func vr(n int) *Expr             { return &Expr{kind: kVar, n: n} }
func addNode(l, r *Expr) *Expr   { return &Expr{kind: kAdd, left: l, right: r} }
func mulNode(l, r *Expr) *Expr   { return &Expr{kind: kMul, left: l, right: r} }
func powNode(l, r *Expr) *Expr   { return &Expr{kind: kPow, left: l, right: r} }
func lnNode(e *Expr) *Expr       { return &Expr{kind: kLn, left: e} }
func isVal(e *Expr) bool         { return e.kind == kVal }
func isValN(e *Expr, n int) bool { return e.kind == kVal && e.n == n }

func pown(a, n int) int {
	if n == 0 {
		return 1
	}
	if n == 1 {
		return a
	}
	b := pown(a, n/2)
	factor := a
	if n%2 == 0 {
		factor = 1
	}
	return b * b * factor
}

func addValLeft(n int, g *Expr) *Expr {
	if isVal(g) {
		return val(n + g.n)
	}
	if n == 0 {
		return g
	}
	if g.kind == kAdd {
		if isVal(g.left) {
			return add(val(n+g.left.n), g.right)
		}
		if isVal(g.right) {
			return add(val(n+g.right.n), g.left)
		}
	}
	return addNode(val(n), g)
}

func addNonValLeft(f, g *Expr) *Expr {
	if isValN(g, 0) {
		return f
	}
	if isVal(g) {
		return add(val(g.n), f)
	}
	if g.kind == kAdd && isVal(g.left) {
		return add(val(g.left.n), add(f, g.right))
	}
	return addNode(f, g)
}

func add(f, g *Expr) *Expr {
	switch f.kind {
	case kVal:
		return addValLeft(f.n, g)
	case kAdd:
		return add(f.left, add(f.right, g))
	}
	return addNonValLeft(f, g)
}

func mulValLeft(n int, g *Expr) *Expr {
	if isVal(g) {
		return val(n * g.n)
	}
	if n == 0 {
		return val(0)
	}
	if n == 1 {
		return g
	}
	if g.kind == kMul {
		if isVal(g.left) {
			return mul(val(n*g.left.n), g.right)
		}
		if isVal(g.right) {
			return mul(val(n*g.right.n), g.left)
		}
	}
	return mulNode(val(n), g)
}

func mulNonValLeft(f, g *Expr) *Expr {
	if isValN(g, 0) {
		return val(0)
	}
	if isValN(g, 1) {
		return f
	}
	if isVal(g) {
		return mul(val(g.n), f)
	}
	if g.kind == kMul && isVal(g.left) {
		return mul(val(g.left.n), mul(f, g.right))
	}
	return mulNode(f, g)
}

func mul(f, g *Expr) *Expr {
	switch f.kind {
	case kVal:
		return mulValLeft(f.n, g)
	case kMul:
		return mul(f.left, mul(f.right, g))
	}
	return mulNonValLeft(f, g)
}

func pow(f, g *Expr) *Expr {
	if isVal(f) {
		m := f.n
		if isVal(g) {
			return val(pown(m, g.n))
		}
		if m == 0 {
			return val(0)
		}
		return powNode(val(m), g)
	}
	if isValN(g, 0) {
		return val(1)
	}
	if isValN(g, 1) {
		return f
	}
	return powNode(f, g)
}

func ln(f *Expr) *Expr {
	if isValN(f, 1) {
		return val(0)
	}
	return lnNode(f)
}

func deriv(x int, expr *Expr) *Expr {
	switch expr.kind {
	case kVal:
		return val(0)
	case kVar:
		if x == expr.n {
			return val(1)
		}
		return val(0)
	case kAdd:
		return add(deriv(x, expr.left), deriv(x, expr.right))
	case kMul:
		f, g := expr.left, expr.right
		return add(mul(f, deriv(x, g)), mul(g, deriv(x, f)))
	case kPow:
		f, g := expr.left, expr.right
		return mul(
			pow(f, g),
			add(
				mul(mul(g, deriv(x, f)), pow(f, val(-1))),
				mul(ln(f), deriv(x, g)),
			),
		)
	}
	f := expr.left
	return mul(deriv(x, f), pow(f, val(-1)))
}

func count(expr *Expr) int {
	switch expr.kind {
	case kVal, kVar:
		return 1
	case kLn:
		return count(expr.left)
	}
	return count(expr.left) + count(expr.right)
}

func main() {
	x := vr(1)
	expr := pow(x, x)
	for step := 0; step < 10; step++ {
		expr = deriv(1, expr)
		fmt.Printf("%d count: %d\n", step+1, count(expr))
	}
}
