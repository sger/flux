#[derive(Clone)]
enum Solution {
    Nil,
    Cons(i64, Box<Solution>),
}

fn safe(queen: i64, diag: i64, xs: &Solution) -> bool {
    match xs {
        Solution::Nil => true,
        Solution::Cons(q, tail) => {
            queen != *q && queen != *q + diag && queen != *q - diag && safe(queen, diag + 1, tail)
        }
    }
}

fn place(n: i64, row: i64, soln: &Solution) -> i64 {
    if row == 0 {
        1
    } else {
        try_col(n, row, soln, n)
    }
}

fn try_col(n: i64, row: i64, soln: &Solution, col: i64) -> i64 {
    if col <= 0 {
        0
    } else if safe(col, 1, soln) {
        place(n, row - 1, &Solution::Cons(col, Box::new(soln.clone())))
            + try_col(n, row, soln, col - 1)
    } else {
        try_col(n, row, soln, col - 1)
    }
}

fn queens(n: i64) -> i64 {
    place(n, n, &Solution::Nil)
}

fn main() {
    println!("{}", queens(13));
}
