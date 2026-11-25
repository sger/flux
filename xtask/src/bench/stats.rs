//! The statistics a benchmark report shows: the Mann–Whitney U test benchstat
//! uses to say whether two sets of timings differ, and a geometric mean.

pub fn mean(xs: &[f64]) -> f64 {
    xs.iter().sum::<f64>() / xs.len().max(1) as f64
}

/// The sample standard deviation; 0 for fewer than two values.
pub fn stddev(xs: &[f64]) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    let m = mean(xs);
    (xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (xs.len() - 1) as f64).sqrt()
}

/// The geometric mean, the average that suits ratios; `None` when empty.
pub fn geomean(ratios: &[f64]) -> Option<f64> {
    if ratios.is_empty() {
        return None;
    }
    Some((ratios.iter().map(|r| r.ln()).sum::<f64>() / ratios.len() as f64).exp())
}

/// The two-sided p-value of the Mann–Whitney U test, from the normal
/// approximation with a tie correction and a continuity correction. It
/// compares ranks, not means, so one slow outlier cannot make a difference
/// on its own. With fewer than four values a side no p falls below 0.05.
pub fn mann_whitney_p(a: &[f64], b: &[f64]) -> f64 {
    let (n1, n2) = (a.len() as f64, b.len() as f64);
    if a.is_empty() || b.is_empty() {
        return 1.0;
    }
    let mut all: Vec<(f64, bool)> =
        a.iter().map(|&x| (x, true)).chain(b.iter().map(|&x| (x, false))).collect();
    all.sort_by(|x, y| x.0.total_cmp(&y.0));

    // Equal values share the average of the ranks they span.
    let mut rank_sum_a = 0.0;
    let mut tie_term = 0.0;
    let mut i = 0;
    while i < all.len() {
        let mut j = i;
        while j + 1 < all.len() && all[j + 1].0 == all[i].0 {
            j += 1;
        }
        let rank = (i + j) as f64 / 2.0 + 1.0;
        let tied = (j - i + 1) as f64;
        tie_term += tied.powi(3) - tied;
        rank_sum_a += rank * all[i..=j].iter().filter(|(_, from_a)| *from_a).count() as f64;
        i = j + 1;
    }

    let n = n1 + n2;
    let u = rank_sum_a - n1 * (n1 + 1.0) / 2.0;
    let mu = n1 * n2 / 2.0;
    let sigma = (n1 * n2 / 12.0 * ((n + 1.0) - tie_term / (n * (n - 1.0)))).sqrt();
    if sigma == 0.0 {
        return 1.0;
    }
    let z = ((u - mu).abs() - 0.5).max(0.0) / sigma;
    erfc(z / std::f64::consts::SQRT_2).min(1.0)
}

/// The complementary error function, by Abramowitz and Stegun 7.1.26
/// (error below 1.5e-7), enough for a p-value.
fn erfc(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
    let poly = t
        * (0.254_829_592
            + t * (-0.284_496_736
                + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let erfc_abs = poly * (-x * x).exp();
    if x >= 0.0 { erfc_abs } else { 2.0 - erfc_abs }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_timings_do_not_differ() {
        let a = [1.0, 1.1, 0.9, 1.05, 0.95];
        assert!(mann_whitney_p(&a, &a) > 0.9);
    }

    #[test]
    fn timings_that_never_overlap_differ() {
        let a: Vec<f64> = (0..10).map(|i| 1.0 + i as f64 * 0.01).collect();
        let b: Vec<f64> = (0..10).map(|i| 2.0 + i as f64 * 0.01).collect();
        assert!(mann_whitney_p(&a, &b) < 0.001);
    }

    #[test]
    fn three_runs_a_side_cannot_reach_significance() {
        assert!(mann_whitney_p(&[1.0, 1.1, 1.2], &[5.0, 5.1, 5.2]) > 0.05);
        assert!(mann_whitney_p(&[1.0, 1.1, 1.2, 1.3], &[5.0, 5.1, 5.2, 5.3]) < 0.05);
    }

    #[test]
    fn erfc_matches_known_values() {
        assert!((erfc(0.0) - 1.0).abs() < 1e-6);
        assert!((erfc(1.0) - 0.157_299_2).abs() < 1e-6);
        assert!((erfc(-1.0) - 1.842_700_8).abs() < 1e-6);
    }

    #[test]
    fn geomean_of_ratios() {
        assert!((geomean(&[2.0, 0.5]).unwrap() - 1.0).abs() < 1e-12);
        assert_eq!(geomean(&[]), None);
        assert!((stddev(&[1.0, 3.0]) - std::f64::consts::SQRT_2).abs() < 1e-12);
    }
}
