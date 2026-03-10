#[derive(Debug)]
enum Expr {
    Var(()),
    Val(i64),
    Add(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
}

const DEPTH: i64 = 12;

fn dec(n: i64) -> i64 {
    if n == 0 { 0 } else { n - 1 }
}

fn mk_expr(n: i64, v: i64) -> Expr {
    if n == 0 {
        if v == 0 { Expr::Var(()) } else { Expr::Val(v) }
    } else {
        Expr::Add(Box::new(mk_expr(n - 1, v + 1)), Box::new(mk_expr(n - 1, dec(v))))
    }
}

fn append_add(expr: Expr, tail: Expr) -> Expr {
    match expr {
        Expr::Add(left, right) => Expr::Add(left, Box::new(append_add(*right, tail))),
        other => Expr::Add(Box::new(other), Box::new(tail)),
    }
}

fn append_mul(expr: Expr, tail: Expr) -> Expr {
    match expr {
        Expr::Mul(left, right) => Expr::Mul(left, Box::new(append_mul(*right, tail))),
        other => Expr::Mul(Box::new(other), Box::new(tail)),
    }
}

fn reassoc(expr: Expr) -> Expr {
    match expr {
        Expr::Add(left, right) => append_add(reassoc(*left), reassoc(*right)),
        Expr::Mul(left, right) => append_mul(reassoc(*left), reassoc(*right)),
        other => other,
    }
}

fn fold_add(left: Expr, right: Expr) -> Expr {
    match (left, right) {
        (Expr::Val(a), Expr::Val(b)) => Expr::Val(a + b),
        (Expr::Val(a), Expr::Add(inner_left, inner_right)) => match (*inner_left, *inner_right) {
            (expr, Expr::Val(b)) => Expr::Add(Box::new(Expr::Val(a + b)), Box::new(expr)),
            (Expr::Val(b), expr) => Expr::Add(Box::new(Expr::Val(a + b)), Box::new(expr)),
            (lhs, rhs) => Expr::Add(
                Box::new(Expr::Val(a)),
                Box::new(Expr::Add(Box::new(lhs), Box::new(rhs))),
            ),
        },
        (lhs, rhs) => Expr::Add(Box::new(lhs), Box::new(rhs)),
    }
}

fn fold_mul(left: Expr, right: Expr) -> Expr {
    match (left, right) {
        (Expr::Val(a), Expr::Val(b)) => Expr::Val(a * b),
        (Expr::Val(a), Expr::Mul(inner_left, inner_right)) => match (*inner_left, *inner_right) {
            (expr, Expr::Val(b)) => Expr::Mul(Box::new(Expr::Val(a * b)), Box::new(expr)),
            (Expr::Val(b), expr) => Expr::Mul(Box::new(Expr::Val(a * b)), Box::new(expr)),
            (lhs, rhs) => Expr::Mul(
                Box::new(Expr::Val(a)),
                Box::new(Expr::Mul(Box::new(lhs), Box::new(rhs))),
            ),
        },
        (lhs, rhs) => Expr::Mul(Box::new(lhs), Box::new(rhs)),
    }
}

fn const_folding(expr: Expr) -> Expr {
    match expr {
        Expr::Add(left, right) => fold_add(const_folding(*left), const_folding(*right)),
        Expr::Mul(left, right) => fold_mul(const_folding(*left), const_folding(*right)),
        other => other,
    }
}

fn eval(expr: &Expr) -> i64 {
    match expr {
        Expr::Var(_) => 0,
        Expr::Val(value) => *value,
        Expr::Add(left, right) => eval(left) + eval(right),
        Expr::Mul(left, right) => eval(left) * eval(right),
    }
}

fn main() {
    let handle = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let original = mk_expr(DEPTH, 1);
            let optimized = const_folding(reassoc(mk_expr(DEPTH, 1)));
            println!("{} {}", eval(&original), eval(&optimized));
        })
        .expect("spawn cfold benchmark");

    handle.join().expect("run cfold benchmark");
}
