type Elem = u32;

const N: usize = 32;

fn bad_rand(seed: Elem) -> Elem {
    seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223)
}

fn mk_random_array(seed: Elem, n: usize) -> Vec<Elem> {
    let mut out = Vec::with_capacity(n);
    let mut cur = seed;
    for _ in 0..n {
        out.push(cur);
        cur = bad_rand(cur);
    }
    out
}

fn check_sorted_checksum(xs: &[Elem]) -> u64 {
    if xs.is_empty() {
        return 0;
    }
    for i in 0..xs.len() - 1 {
        assert!(xs[i] <= xs[i + 1], "array is not sorted");
    }
    xs[xs.len() - 1] as u64 + xs.len() as u64
}

fn partition(xs: &mut [Elem], lo: usize, hi: usize) -> usize {
    let mid = (lo + hi) / 2;
    if xs[mid] < xs[lo] {
        xs.swap(lo, mid);
    }
    if xs[hi] < xs[lo] {
        xs.swap(lo, hi);
    }
    if xs[mid] < xs[hi] {
        xs.swap(mid, hi);
    }
    let pivot = xs[hi];
    let mut i = lo;
    let mut j = lo;
    while j < hi {
        if xs[j] < pivot {
            xs.swap(i, j);
            i += 1;
        }
        j += 1;
    }
    xs.swap(i, hi);
    i
}

fn qsort_aux(xs: &mut [Elem], lo: usize, hi: usize) {
    if lo < hi {
        let mid = partition(xs, lo, hi);
        if mid > 0 {
            qsort_aux(xs, lo, mid - 1);
        }
        qsort_aux(xs, mid + 1, hi);
    }
}

fn qsort(xs: &mut [Elem]) {
    if !xs.is_empty() {
        qsort_aux(xs, 0, xs.len() - 1);
    }
}

fn sort_and_checksum(i: usize) -> u64 {
    let mut xs = mk_random_array(i as Elem, i);
    qsort(&mut xs);
    check_sorted_checksum(&xs)
}

fn bench() -> u64 {
    let mut acc = 0_u64;
    for _ in 0..N {
        for i in 0..N {
            acc += sort_and_checksum(i);
        }
    }
    acc
}

fn main() {
    println!("{}", bench());
}
