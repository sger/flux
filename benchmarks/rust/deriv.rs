use std::env;
use std::rc::Rc;

// Subterms are shared, as in the Flux version: `deriv` uses `f` and `g` more
// than once, and a clone of an `Expr` copies one node, not the whole tree.
#[derive(Clone)]
enum Expr {
    Val(i64),
    Var(i64),
    Add(Rc<Expr>, Rc<Expr>),
    Mul(Rc<Expr>, Rc<Expr>),
    Pow(Rc<Expr>, Rc<Expr>),
    Ln(Rc<Expr>),
}

/// Takes the node out when this is its only reference, as Perceus reuses a
/// unique cell, and copies just that node when it is shared.
fn take(expr: Rc<Expr>) -> Expr {
    Rc::try_unwrap(expr).unwrap_or_else(|shared| (*shared).clone())
}

fn pown(a: i64, n: i64) -> i64 {
    if n == 0 {
        1
    } else if n == 1 {
        a
    } else {
        let b = pown(a, n / 2);
        let factor = if n % 2 == 0 { 1 } else { a };
        b * b * factor
    }
}

fn add(f: Expr, g: Expr) -> Expr {
    match f {
        Expr::Val(n) => match g {
            Expr::Val(m) => Expr::Val(n + m),
            other => {
                if n == 0 {
                    other
                } else {
                    match other {
                        Expr::Add(left, right) => match (take(left), take(right)) {
                            (Expr::Val(m), tail) => add(Expr::Val(n + m), tail),
                            (head, Expr::Val(m)) => add(Expr::Val(n + m), head),
                            (lhs, rhs) => Expr::Add(
                                Rc::new(Expr::Val(n)),
                                Rc::new(Expr::Add(Rc::new(lhs), Rc::new(rhs))),
                            ),
                        },
                        other => Expr::Add(Rc::new(Expr::Val(n)), Rc::new(other)),
                    }
                }
            }
        },
        Expr::Add(left, right) => add(take(left), add(take(right), g)),
        other => match g {
            Expr::Val(0) => other,
            Expr::Val(n) => add(Expr::Val(n), other),
            Expr::Add(left, right) => match (take(left), take(right)) {
                (Expr::Val(n), tail) => add(Expr::Val(n), add(other, tail)),
                (head, tail) => {
                    Expr::Add(Rc::new(other), Rc::new(Expr::Add(Rc::new(head), Rc::new(tail))))
                }
            },
            rhs => Expr::Add(Rc::new(other), Rc::new(rhs)),
        },
    }
}

fn mul(f: Expr, g: Expr) -> Expr {
    match f {
        Expr::Val(n) => match g {
            Expr::Val(m) => Expr::Val(n * m),
            other => {
                if n == 0 {
                    Expr::Val(0)
                } else if n == 1 {
                    other
                } else {
                    match other {
                        Expr::Mul(left, right) => match (take(left), take(right)) {
                            (Expr::Val(m), tail) => mul(Expr::Val(n * m), tail),
                            (head, Expr::Val(m)) => mul(Expr::Val(n * m), head),
                            (lhs, rhs) => Expr::Mul(
                                Rc::new(Expr::Val(n)),
                                Rc::new(Expr::Mul(Rc::new(lhs), Rc::new(rhs))),
                            ),
                        },
                        other => Expr::Mul(Rc::new(Expr::Val(n)), Rc::new(other)),
                    }
                }
            }
        },
        Expr::Mul(left, right) => mul(take(left), mul(take(right), g)),
        other => match g {
            Expr::Val(0) => Expr::Val(0),
            Expr::Val(1) => other,
            Expr::Val(n) => mul(Expr::Val(n), other),
            Expr::Mul(left, right) => match (take(left), take(right)) {
                (Expr::Val(n), tail) => mul(Expr::Val(n), mul(other, tail)),
                (head, tail) => {
                    Expr::Mul(Rc::new(other), Rc::new(Expr::Mul(Rc::new(head), Rc::new(tail))))
                }
            },
            rhs => Expr::Mul(Rc::new(other), Rc::new(rhs)),
        },
    }
}

fn pow(f: Expr, g: Expr) -> Expr {
    match f {
        Expr::Val(m) => match g {
            Expr::Val(n) => Expr::Val(pown(m, n)),
            other => {
                if m == 0 {
                    Expr::Val(0)
                } else {
                    match other {
                        Expr::Val(0) => Expr::Val(1),
                        Expr::Val(1) => Expr::Val(m),
                        other => Expr::Pow(Rc::new(Expr::Val(m)), Rc::new(other)),
                    }
                }
            }
        },
        other => match g {
            Expr::Val(0) => Expr::Val(1),
            Expr::Val(1) => other,
            rhs => match other {
                Expr::Val(0) => Expr::Val(0),
                lhs => Expr::Pow(Rc::new(lhs), Rc::new(rhs)),
            },
        },
    }
}

fn ln(f: Expr) -> Expr {
    match f {
        Expr::Val(1) => Expr::Val(0),
        other => Expr::Ln(Rc::new(other)),
    }
}

fn deriv(x: i64, expr: Expr) -> Expr {
    match expr {
        Expr::Val(_) => Expr::Val(0),
        Expr::Var(y) => {
            if x == y {
                Expr::Val(1)
            } else {
                Expr::Val(0)
            }
        }
        Expr::Add(f, g) => add(deriv(x, take(f)), deriv(x, take(g))),
        Expr::Mul(f, g) => {
            let f_expr = take(f);
            let g_expr = take(g);
            add(mul(f_expr.clone(), deriv(x, g_expr.clone())), mul(g_expr, deriv(x, f_expr)))
        }
        Expr::Pow(f, g) => {
            let f_expr = take(f);
            let g_expr = take(g);
            mul(
                pow(f_expr.clone(), g_expr.clone()),
                add(
                    mul(
                        mul(g_expr.clone(), deriv(x, f_expr.clone())),
                        pow(f_expr.clone(), Expr::Val(-1)),
                    ),
                    mul(ln(f_expr), deriv(x, g_expr)),
                ),
            )
        }
        Expr::Ln(f) => {
            let f_expr = take(f);
            mul(deriv(x, f_expr.clone()), pow(f_expr, Expr::Val(-1)))
        }
    }
}

fn count(expr: &Expr) -> usize {
    match expr {
        Expr::Val(_) | Expr::Var(_) => 1,
        Expr::Add(f, g) | Expr::Mul(f, g) | Expr::Pow(f, g) => count(f) + count(g),
        Expr::Ln(f) => count(f),
    }
}

fn main() {
    let steps = env::args().nth(1).and_then(|value| value.parse::<usize>().ok()).unwrap_or(10);
    let mut expr = pow(Expr::Var(1), Expr::Var(1));
    for step in 0..steps {
        expr = deriv(1, expr);
        println!("{} count: {}", step + 1, count(&expr));
    }
}
