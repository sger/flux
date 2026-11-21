//! Shuffled internal order (proposal 0190 §9), as Go's `ssa/check/seed` does.
//!
//! With `--shuffle-seed <n>` (or `FLUX_SHUFFLE_SEED`), the compiler visits, in
//! an order taken from the seed, what it iterates in an order that should not
//! matter: the definitions each pass is handed ([`visit_order`]), and the
//! entries of a map whose order reaches output ([`ordered`]). Without a seed
//! the order is the program's, or the keys', so output is unchanged.
//!
//! std's `HashMap` already varies its order from one process to the next, so
//! hash-order bugs show without this. A seed varies the orders that are fixed
//! otherwise, and does it the same way on every run with that seed, so a
//! difference it finds can be replayed. A visit order may change the numbers
//! the compiler hands out, never the program: compare a shuffled run by its
//! normalized dump hash (`cargo xtask sweep --determinism --shuffle`).

use std::sync::OnceLock;

/// `None`: no shuffling.
static SEED: OnceLock<Option<u64>> = OnceLock::new();

#[cfg(test)]
thread_local! {
    /// A seed for this test's thread only, since unit tests run in parallel.
    static TEST_SEED: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// Sets the seed from `--shuffle-seed`, falling back to `FLUX_SHUFFLE_SEED`.
pub fn configure(cli_seed: Option<u64>) {
    SEED.get_or_init(|| cli_seed.or_else(env_seed));
}

fn env_seed() -> Option<u64> {
    std::env::var("FLUX_SHUFFLE_SEED").ok()?.trim().parse().ok()
}

/// The seed in force, if any.
pub fn seed() -> Option<u64> {
    #[cfg(test)]
    if let Some(seed) = TEST_SEED.with(std::cell::Cell::get) {
        return Some(seed);
    }
    *SEED.get_or_init(env_seed)
}

/// Whether a seed is set.
pub fn is_active() -> bool {
    seed().is_some()
}

/// The order to visit `len` items in: `0..len`, or under a seed a permutation
/// of it. `salt` names the loop, so each loop gets its own order. Bisection
/// numbers rewrites in definition order, so it turns shuffling off.
pub fn visit_order(len: usize, salt: &str) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    if let Some(seed) = seed()
        && !crate::shared::opt_bisect::is_active()
    {
        permute(&mut order, seed ^ fnv1a(salt));
    }
    order
}

/// A map's entries in key order, or under a seed in an order taken from it.
/// Sorted first either way, so the result never follows the map's hash order.
pub fn ordered<K: Ord, V>(entries: impl IntoIterator<Item = (K, V)>, salt: &str) -> Vec<(K, V)> {
    let mut entries: Vec<(K, V)> = entries.into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some(seed) = seed() {
        permute(&mut entries, seed ^ fnv1a(salt));
    }
    entries
}

/// Fisher–Yates, driven by splitmix64.
fn permute<T>(items: &mut [T], seed: u64) {
    let mut state = seed;
    for i in (1..items.len()).rev() {
        let j = (splitmix64(&mut state) % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
pub(crate) fn with_seed<R>(seed: u64, f: impl FnOnce() -> R) -> R {
    TEST_SEED.with(|cell| cell.set(Some(seed)));
    let result = f();
    TEST_SEED.with(|cell| cell.set(None));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_seed_the_order_is_unchanged() {
        assert_eq!(visit_order(5, "pass"), vec![0, 1, 2, 3, 4]);
        assert_eq!(
            ordered(vec![(3, 'c'), (1, 'a'), (2, 'b')], "map"),
            vec![(1, 'a'), (2, 'b'), (3, 'c')]
        );
    }

    #[test]
    fn a_seed_permutes_every_index_once() {
        let order = with_seed(7, || visit_order(50, "pass"));
        assert_ne!(order, (0..50).collect::<Vec<_>>());
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..50).collect::<Vec<_>>());
    }

    #[test]
    fn a_seed_and_salt_give_the_same_order_every_time() {
        let first = with_seed(7, || visit_order(50, "pass"));
        let second = with_seed(7, || visit_order(50, "pass"));
        assert_eq!(first, second);
    }

    #[test]
    fn each_salt_and_each_seed_gets_its_own_order() {
        let base = with_seed(7, || visit_order(50, "pass"));
        assert_ne!(base, with_seed(7, || visit_order(50, "other")));
        assert_ne!(base, with_seed(8, || visit_order(50, "pass")));
    }

    #[test]
    fn ordered_ignores_the_input_order_under_a_seed() {
        let a = with_seed(3, || ordered(vec![(1, ()), (2, ()), (3, ()), (4, ())], "m"));
        let b = with_seed(3, || ordered(vec![(4, ()), (2, ()), (3, ()), (1, ())], "m"));
        assert_eq!(a, b);
    }
}
