//! A function that needs more than 256 local slots runs correctly on the VM
//! (KI-127).
//!
//! The bytecode emitter gives each `let` a slot of its own, and its
//! intermediate results too, so a function with about ninety `let`s already
//! needs more than 256. Local instructions named their slot in one byte, and
//! slot 256 wrapped to slot 0, which holds the first argument: every later
//! read of it saw another value, and the result was wrong with no error.
//!
//! Each program is generated with an argument the compiler cannot fold
//! (`len([1, 2])`), and its expected output is computed here.

use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

/// Runs `source` on the VM with and without `--optimize`, and returns each
/// way whose output was not `expected`.
fn mismatches(label: &str, source: &str, expected: i64) -> Vec<String> {
    let scratch = Scratch::new(label);
    let path = scratch.write(&format!("{label}.flx"), source);
    let mut wrong = Vec::new();
    for way in [&["--no-cache"][..], &["--no-cache", "--optimize"][..]] {
        let output = Command::new(env!("CARGO_BIN_EXE_flux"))
            .current_dir(scratch::workspace_root())
            .arg(&path)
            .args(way)
            .args(scratch.cache_args())
            .output()
            .unwrap_or_else(|e| panic!("failed to run flux on {label}: {e}"));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let printed = stdout.lines().map(str::trim).rfind(|line| !line.is_empty());
        if !output.status.success() || printed != Some(expected.to_string().as_str()) {
            let stderr = String::from_utf8_lossy(&output.stderr);
            wrong.push(format!(
                "{label} {way:?}: expected {expected}, got {printed:?}\n{}",
                stderr
                    .lines()
                    .filter(|line| !line.trim_start().starts_with('['))
                    .take(12)
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
    }
    wrong
}

/// `let v{i} = n + {i}` for each `i` below `count`, one per line.
fn lets(count: usize) -> String {
    (0..count).map(|i| format!("    let v{i} = n + {i}\n")).collect()
}

fn sum(count: usize) -> String {
    (0..count).map(|i| format!("v{i}")).collect::<Vec<_>>().join(" + ")
}

/// The sum of `n + i` for each `i` below `count`, with `n` = 2.
fn expected_sum(count: usize) -> i64 {
    let count = count as i64;
    2 * count + count * (count - 1) / 2
}

#[test]
fn a_function_with_hundreds_of_lets_computes_the_right_result() {
    let mut wrong = Vec::new();
    for count in [120, 300] {
        let source = format!(
            "fn total(n) {{\n{}    {}\n}}\n\nfn main() {{\n    print(total(len([1, 2])))\n}}\n",
            lets(count),
            sum(count)
        );
        wrong.extend(mismatches(&format!("lets_{count}"), &source, expected_sum(count)));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n\n"));
}

/// The closure's body holds every sum: the simplifier copies each cheap
/// `n + i` into it rather than capturing the value. It is passed to a
/// recursive `apply`, not returned: the VM's check on a returned closure
/// fails on its own (KI-111).
#[test]
fn a_closure_with_hundreds_of_lets_computes_the_right_result() {
    let mut wrong = Vec::new();
    for count in [254, 300] {
        let source = format!(
            "fn apply(k, f) {{ if k < 0 {{ apply(k + 1, f) }} else {{ f(1) }} }}\n\n\
             fn total(n) {{\n{}    apply(n, \\x -> x + {})\n}}\n\n\
             fn main() {{\n    print(total(len([1, 2])))\n}}\n",
            lets(count),
            sum(count)
        );
        wrong.extend(mismatches(&format!("closure_{count}"), &source, 1 + expected_sum(count)));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n\n"));
}

/// The emitter fuses a local read into indexing, calling and constructor
/// matching, and those fused instructions also named their slot in one byte.
/// The helpers are recursive so they are not inlined and their results stay
/// in slots of their own, after the 300 sums.
#[test]
fn a_value_past_the_256th_slot_can_be_indexed_called_and_matched() {
    let count = 300;
    let source = format!(
        "data Box {{ Box(Int) }}\n\n\
         fn keep(k) {{ if k < 0 {{ keep(k + 1) }} else {{ k }} }}\n\
         fn wrap(k) {{ if k < 0 {{ wrap(k + 1) }} else {{ Some(k) }} }}\n\
         fn boxed(k) {{ if k < 0 {{ boxed(k + 1) }} else {{ Box(k) }} }}\n\
         fn choose(k, f) {{ if k < 0 {{ choose(k + 1, f) }} else {{ f }} }}\n\n\
         fn total(n: Int, f: (Int) -> Int) -> Int {{\n{}    let s = {}\n\
         \x20   let xs = [keep(n), 5]\n\
         \x20   let g = choose(n, f)\n\
         \x20   let o = wrap(n)\n\
         \x20   let b = boxed(n)\n\
         \x20   let picked = match xs[0] {{ Some(v) -> v, None -> 0 }}\n\
         \x20   let opt = match o {{ Some(v) -> v, None -> 0 }}\n\
         \x20   let unboxed = match b {{ Box(v) -> v }}\n\
         \x20   s + picked + g(n) + opt + unboxed\n}}\n\n\
         fn main() {{\n    print(total(len([1, 2]), \\x -> x + 10))\n}}\n",
        lets(count),
        sum(count)
    );
    // `picked`, `opt` and `unboxed` are each 2, and `g(n)` is 12.
    let wrong = mismatches("past_slot_255", &source, expected_sum(count) + 18);
    assert!(wrong.is_empty(), "{}", wrong.join("\n\n"));
}

/// A call counted its arguments in one byte too. `count` calls itself in tail
/// position and `plus` calls it outside one, so each kind of call is covered.
#[test]
fn a_call_with_hundreds_of_arguments_computes_the_right_result() {
    let count = 300;
    let params = (0..count).map(|i| format!("a{i}")).collect::<Vec<_>>().join(", ");
    let rest = (1..count).map(|i| format!("a{i}")).collect::<Vec<_>>().join(", ");
    let args = (0..count).map(|i| format!("n + {i}")).collect::<Vec<_>>().join(", ");
    let last = count - 1;
    let source = format!(
        "fn first({params}) {{ a0 + a{last} }}\n\n\
         fn count({params}) {{ if a0 > 5 {{ a0 + a{last} }} else {{ count(a0 + 1, {rest}) }} }}\n\n\
         fn plus({params}) {{ count({params}) + 1 }}\n\n\
         fn main() {{\n    let n = len([1, 2])\n    \
         print(first({args}) + count({args}) + plus({args}))\n}}\n"
    );
    // `first` is 2 + 301, `count` raises `a0` to 6 and adds 301, and `plus`
    // is one more than that.
    let wrong = mismatches("call_300", &source, 303 + 307 + 308);
    assert!(wrong.is_empty(), "{}", wrong.join("\n\n"));
}
