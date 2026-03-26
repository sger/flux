use std::rc::Rc;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use flux::runtime::cons_cell::ConsCell;
use flux::runtime::value::Value;

/// Benchmark fresh cons cell allocation: builds a list of N elements.
fn bench_cons_fresh_alloc(c: &mut Criterion) {
    let mut group = c.benchmark_group("aether/cons_fresh");

    for &size in &[100, 1_000, 10_000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &n| {
            b.iter(|| {
                let mut list = Value::EmptyList;
                for i in 0..n {
                    list = ConsCell::cons(Value::Integer(i as i64), list);
                }
                black_box(list);
            });
        });
    }

    group.finish();
}

/// Benchmark reuse path: when we have a unique Rc<ConsCell>, rewriting its
/// fields in-place avoids a fresh allocation.
fn bench_cons_reuse(c: &mut Criterion) {
    let mut group = c.benchmark_group("aether/cons_reuse");

    for &size in &[100, 1_000, 10_000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &n| {
            b.iter(|| {
                // Build a list, then "map" over it reusing each cell in-place.
                // This simulates the Aether reuse pattern: drop old cell, reuse
                // its allocation for the new cell.
                let mut list = Value::EmptyList;
                for i in 0..n {
                    list = ConsCell::cons(Value::Integer(i as i64), list);
                }

                // Simulate a map: walk the list, reuse each unique cell
                let mut cur = list;
                let mut result = Value::EmptyList;
                while let Value::Cons(rc) = cur {
                    match Rc::try_unwrap(rc) {
                        Ok(mut cell) => {
                            cur = std::mem::replace(&mut cell.tail, Value::EmptyList);
                            // Reuse: modify head in place, set new tail
                            if let Value::Integer(n) = &cell.head {
                                cell.head = Value::Integer(n * 2);
                            }
                            cell.tail = result;
                            result = Value::Cons(Rc::new(cell));
                        }
                        Err(rc) => {
                            // Shared: allocate fresh
                            cur = rc.tail.clone();
                            let new_head = if let Value::Integer(n) = &rc.head {
                                Value::Integer(n * 2)
                            } else {
                                rc.head.clone()
                            };
                            result = ConsCell::cons(new_head, result);
                        }
                    }
                }
                black_box(result);
            });
        });
    }

    group.finish();
}

/// Benchmark fresh allocation map: same traversal but always allocates fresh.
fn bench_cons_map_fresh(c: &mut Criterion) {
    let mut group = c.benchmark_group("aether/cons_map_fresh");

    for &size in &[100, 1_000, 10_000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &n| {
            b.iter(|| {
                // Build a list
                let mut list = Value::EmptyList;
                for i in 0..n {
                    list = ConsCell::cons(Value::Integer(i as i64), list);
                }

                // Map without reuse: always allocate fresh
                let mut cur = list;
                let mut result = Value::EmptyList;
                while let Value::Cons(rc) = cur {
                    cur = rc.tail.clone();
                    let new_head = if let Value::Integer(n) = &rc.head {
                        Value::Integer(n * 2)
                    } else {
                        rc.head.clone()
                    };
                    result = ConsCell::cons(new_head, result);
                }
                black_box(result);
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_cons_fresh_alloc,
    bench_cons_reuse,
    bench_cons_map_fresh
);
criterion_main!(benches);
