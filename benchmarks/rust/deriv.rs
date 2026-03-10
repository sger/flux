#[derive(Clone)]
enum Expr {
    Val(i64),
    Var(i64),
    Add(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Pow(Box<Expr>, Box<Expr>),
    Ln(Box<Expr>),
}

const STEPS: usize = 10;

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
                        Expr::Add(left, right) => match (*left, *right) {
                            (Expr::Val(m), tail) => add(Expr::Val(n + m), tail),
                            (head, Expr::Val(m)) => add(Expr::Val(n + m), head),
                            (lhs, rhs) => Expr::Add(
                                Box::new(Expr::Val(n)),
                                Box::new(Expr::Add(Box::new(lhs), Box::new(rhs))),
                            ),
                        },
                        other => Expr::Add(Box::new(Expr::Val(n)), Box::new(other)),
                    }
                }
            }
        },
        Expr::Add(left, right) => add(*left, add(*right, g)),
        other => match g {
            Expr::Val(0) => other,
            Expr::Val(n) => add(Expr::Val(n), other),
            Expr::Add(left, right) => match (*left, *right) {
                (Expr::Val(n), tail) => add(Expr::Val(n), add(other, tail)),
                (head, tail) => Expr::Add(
                    Box::new(other),
                    Box::new(Expr::Add(Box::new(head), Box::new(tail))),
                ),
            },
            rhs => Expr::Add(Box::new(other), Box::new(rhs)),
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
                        Expr::Mul(left, right) => match (*left, *right) {
                            (Expr::Val(m), tail) => mul(Expr::Val(n * m), tail),
                            (head, Expr::Val(m)) => mul(Expr::Val(n * m), head),
                            (lhs, rhs) => Expr::Mul(
                                Box::new(Expr::Val(n)),
                                Box::new(Expr::Mul(Box::new(lhs), Box::new(rhs))),
                            ),
                        },
                        other => Expr::Mul(Box::new(Expr::Val(n)), Box::new(other)),
                    }
                }
            }
        },
        Expr::Mul(left, right) => mul(*left, mul(*right, g)),
        other => match g {
            Expr::Val(0) => Expr::Val(0),
            Expr::Val(1) => other,
            Expr::Val(n) => mul(Expr::Val(n), other),
            Expr::Mul(left, right) => match (*left, *right) {
                (Expr::Val(n), tail) => mul(Expr::Val(n), mul(other, tail)),
                (head, tail) => Expr::Mul(
                    Box::new(other),
                    Box::new(Expr::Mul(Box::new(head), Box::new(tail))),
                ),
            },
            rhs => Expr::Mul(Box::new(other), Box::new(rhs)),
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
                        other => Expr::Pow(Box::new(Expr::Val(m)), Box::new(other)),
                    }
                }
            }
        },
        other => match g {
            Expr::Val(0) => Expr::Val(1),
            Expr::Val(1) => other,
            rhs => match other {
                Expr::Val(0) => Expr::Val(0),
                lhs => Expr::Pow(Box::new(lhs), Box::new(rhs)),
            },
        },
    }
}

fn ln(f: Expr) -> Expr {
    match f {
        Expr::Val(1) => Expr::Val(0),
        other => Expr::Ln(Box::new(other)),
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
        Expr::Add(f, g) => add(deriv(x, *f), deriv(x, *g)),
        Expr::Mul(f, g) => {
            let f_expr = *f;
            let g_expr = *g;
            add(
                mul(f_expr.clone(), deriv(x, g_expr.clone())),
                mul(g_expr, deriv(x, f_expr)),
            )
        }
        Expr::Pow(f, g) => {
            let f_expr = *f;
            let g_expr = *g;
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
            let f_expr = *f;
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
    let mut expr = pow(Expr::Var(1), Expr::Var(1));
    for step in 0..STEPS {
        expr = deriv(1, expr);
        println!("{} count: {}", step + 1, count(&expr));
    }
}
