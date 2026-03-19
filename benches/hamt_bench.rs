use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use flux::runtime::hamt::{hamt_empty, hamt_insert, hamt_lookup};
use flux::runtime::hash_key::HashKey;
use flux::runtime::value::Value;

fn bench_hamt_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("hamt/insert");

    for &size in &[100, 1_000, 10_000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &n| {
            b.iter(|| {
                let mut root = hamt_empty();
                for i in 0..n {
                    root = hamt_insert(
                        &root,
                        HashKey::Integer(i as i64),
                        Value::Integer(i as i64),
                    );
                }
                black_box(root);
            });
        });
    }

    group.finish();
}

fn bench_hamt_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("hamt/lookup");

    for &size in &[100, 1_000, 10_000] {
        // Pre-build the map
        let mut root = hamt_empty();
        for i in 0..size {
            root = hamt_insert(
                &root,
                HashKey::Integer(i as i64),
                Value::Integer(i as i64),
            );
        }

        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &n| {
            b.iter(|| {
                for i in 0..n {
                    black_box(hamt_lookup(&root, &HashKey::Integer(i as i64)));
                }
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_hamt_insert, bench_hamt_lookup);
criterion_main!(benches);
