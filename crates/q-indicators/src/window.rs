//! Pandas FixedWindowIndexer rolling primitives (mean, var, std).
//!
//! These `pub(crate)` kernels are only called from unit tests until public
//! indicator wrappers land; keep them despite crate-level dead_code.

#![allow(dead_code)]
#![allow(clippy::needless_range_loop)]

use crate::ieee::{ieee_eq, window_missing};

/// Map ±inf to NaN the way pandas `BaseWindow._prep_values` does.
fn prep_values(values: &[f64]) -> Vec<f64> {
    values
        .iter()
        .map(|&x| if window_missing(x) { f64::NAN } else { x })
        .collect()
}

fn fixed_bounds(i: usize, window: usize) -> (usize, usize) {
    let end = i + 1;
    let start = end.saturating_sub(window);
    (start, end)
}

fn calc_mean(
    minp: usize,
    nobs: usize,
    neg_ct: usize,
    sum_x: f64,
    num_consecutive_same_value: i64,
    prev_value: f64,
) -> f64 {
    if nobs >= minp && nobs > 0 {
        let mut result = sum_x / (nobs as f64);
        if num_consecutive_same_value >= nobs as i64 {
            result = prev_value;
        } else if (neg_ct == 0 && result < 0.0) || (neg_ct == nobs && result > 0.0) {
            // Signbit clamp: all-positive must not go negative, and vice versa.
            result = 0.0;
        }
        result
    } else {
        f64::NAN
    }
}

fn add_mean(
    val: f64,
    nobs: &mut usize,
    sum_x: &mut f64,
    neg_ct: &mut usize,
    compensation: &mut f64,
    num_consecutive_same_value: &mut i64,
    prev_value: &mut f64,
) {
    // Not NaN (C ==)
    if ieee_eq(val, val) {
        *nobs += 1;
        let y = val - *compensation;
        let t = *sum_x + y;
        *compensation = t - *sum_x - y;
        *sum_x = t;
        if val.is_sign_negative() {
            *neg_ct += 1;
        }
        if ieee_eq(val, *prev_value) {
            *num_consecutive_same_value += 1;
        } else {
            *num_consecutive_same_value = 1;
            *prev_value = val;
        }
    }
}

fn remove_mean(
    val: f64,
    nobs: &mut usize,
    sum_x: &mut f64,
    neg_ct: &mut usize,
    compensation: &mut f64,
) {
    if ieee_eq(val, val) {
        *nobs -= 1;
        let y = -val - *compensation;
        let t = *sum_x + y;
        *compensation = t - *sum_x - y;
        *sum_x = t;
        if val.is_sign_negative() {
            *neg_ct -= 1;
        }
    }
}

/// Pandas `roll_mean` with FixedWindowIndexer bounds and `min_periods = window`.
pub(crate) fn rolling_mean(values: &[f64], window: usize) -> Vec<f64> {
    let n = values.len();
    let mut output = vec![f64::NAN; n];
    if n == 0 {
        return output;
    }
    let values = prep_values(values);
    let minp = window;
    // Fixed monotonic windows: always true for FixedWindowIndexer.
    let is_monotonic_increasing_bounds = true;

    let mut compensation_add = 0.0;
    let mut compensation_remove = 0.0;
    let mut sum_x = 0.0;
    let mut nobs: usize = 0;
    let mut neg_ct: usize = 0;
    let mut prev_value = 0.0;
    let mut num_consecutive_same_value: i64 = 0;
    let mut prev_start = 0usize;
    let mut prev_end = 0usize;

    for i in 0..n {
        let (s, e) = fixed_bounds(i, window);

        if i == 0 || !is_monotonic_increasing_bounds || s >= prev_end {
            compensation_add = 0.0;
            compensation_remove = 0.0;
            sum_x = 0.0;
            nobs = 0;
            neg_ct = 0;
            // Guard values[s] when the window is empty (window 0) or s is past the end.
            prev_value = if s < e && s < values.len() {
                values[s]
            } else {
                0.0
            };
            num_consecutive_same_value = 0;
            for j in s..e {
                add_mean(
                    values[j],
                    &mut nobs,
                    &mut sum_x,
                    &mut neg_ct,
                    &mut compensation_add,
                    &mut num_consecutive_same_value,
                    &mut prev_value,
                );
            }
        } else {
            for j in prev_start..s {
                remove_mean(
                    values[j],
                    &mut nobs,
                    &mut sum_x,
                    &mut neg_ct,
                    &mut compensation_remove,
                );
            }
            for j in prev_end..e {
                add_mean(
                    values[j],
                    &mut nobs,
                    &mut sum_x,
                    &mut neg_ct,
                    &mut compensation_add,
                    &mut num_consecutive_same_value,
                    &mut prev_value,
                );
            }
        }

        output[i] = calc_mean(
            minp,
            nobs,
            neg_ct,
            sum_x,
            num_consecutive_same_value,
            prev_value,
        );

        if !is_monotonic_increasing_bounds {
            nobs = 0;
            neg_ct = 0;
            sum_x = 0.0;
            compensation_remove = 0.0;
        }

        prev_start = s;
        prev_end = e;
    }

    output
}

const INV_COND_TOL: f64 = f64::EPSILON * 1e3;

fn calc_var(minp: usize, ddof: i32, nobs: f64, ssqdm_x: f64) -> f64 {
    if nobs >= minp as f64 && nobs > f64::from(ddof) {
        ssqdm_x / (nobs - f64::from(ddof))
    } else {
        f64::NAN
    }
}

#[expect(
    clippy::suboptimal_flops,
    reason = "pandas evaluates a*b+c unfused; mul_add changes the result bits"
)]
fn add_var(
    val: f64,
    nobs: &mut f64,
    mean_x: &mut f64,
    ssqdm_x: &mut f64,
    compensation: &mut f64,
    numerically_unstable: &mut bool,
) {
    // GH#21813: use != NaN check via C inequality
    if !ieee_eq(val, val) {
        return;
    }

    *nobs += 1.0;

    let prev_m2 = *ssqdm_x;
    let prev_mean = *mean_x - *compensation;
    let y = val - *compensation;
    let t = y - *mean_x;
    *compensation = t + *mean_x - y;
    let delta = t;
    if *nobs != 0.0 {
        *mean_x += delta / *nobs;
    } else {
        *mean_x = 0.0;
    }
    *ssqdm_x += (val - prev_mean) * (val - *mean_x);

    if prev_m2 * INV_COND_TOL > *ssqdm_x {
        *numerically_unstable = true;
    }
}

#[expect(
    clippy::suboptimal_flops,
    reason = "pandas evaluates a*b+c unfused; mul_add changes the result bits"
)]
fn remove_var(
    val: f64,
    nobs: &mut f64,
    mean_x: &mut f64,
    ssqdm_x: &mut f64,
    compensation: &mut f64,
    numerically_unstable: &mut bool,
) {
    if ieee_eq(val, val) {
        *nobs -= 1.0;
        if *nobs != 0.0 {
            let prev_m2 = *ssqdm_x;
            let prev_mean = *mean_x - *compensation;
            let y = val - *compensation;
            let t = y - *mean_x;
            *compensation = t + *mean_x - y;
            let delta = t;
            *mean_x -= delta / *nobs;
            *ssqdm_x -= (val - prev_mean) * (val - *mean_x);

            if prev_m2 * INV_COND_TOL > *ssqdm_x {
                *numerically_unstable = true;
            }
        } else {
            *mean_x = 0.0;
            *ssqdm_x = 0.0;
            *numerically_unstable = false;
        }
    }
}

/// Pandas `roll_var` with FixedWindowIndexer, `ddof = 1`, `min_periods = max(window, 1)`.
pub(crate) fn rolling_var(values: &[f64], window: usize) -> Vec<f64> {
    let n = values.len();
    let mut output = vec![f64::NAN; n];
    if n == 0 {
        return output;
    }
    let values = prep_values(values);
    let minp = window.max(1);
    let ddof = 1;
    let is_monotonic_increasing_bounds = true;

    let mut mean_x = 0.0;
    let mut ssqdm_x = 0.0;
    let mut nobs = 0.0;
    let mut compensation_add = 0.0;
    let mut compensation_remove = 0.0;
    let mut numerically_unstable = false;
    let mut prev_start = 0usize;
    let mut prev_end = 0usize;

    for i in 0..n {
        let (s, e) = fixed_bounds(i, window);

        let requires_recompute = i == 0 || !is_monotonic_increasing_bounds || s >= prev_end;

        if !requires_recompute {
            for j in prev_start..s {
                remove_var(
                    values[j],
                    &mut nobs,
                    &mut mean_x,
                    &mut ssqdm_x,
                    &mut compensation_remove,
                    &mut numerically_unstable,
                );
            }
            for j in prev_end..e {
                add_var(
                    values[j],
                    &mut nobs,
                    &mut mean_x,
                    &mut ssqdm_x,
                    &mut compensation_add,
                    &mut numerically_unstable,
                );
            }
        }

        if requires_recompute || numerically_unstable {
            mean_x = 0.0;
            ssqdm_x = 0.0;
            nobs = 0.0;
            compensation_add = 0.0;
            compensation_remove = 0.0;
            for j in s..e {
                add_var(
                    values[j],
                    &mut nobs,
                    &mut mean_x,
                    &mut ssqdm_x,
                    &mut compensation_add,
                    &mut numerically_unstable,
                );
            }
            numerically_unstable = false;
        }

        output[i] = calc_var(minp, ddof, nobs, ssqdm_x);

        if !is_monotonic_increasing_bounds {
            nobs = 0.0;
            mean_x = 0.0;
            ssqdm_x = 0.0;
            compensation_remove = 0.0;
        }

        prev_start = s;
        prev_end = e;
    }

    output
}

/// `zsqrt(rolling_var)`: sqrt with negatives clamped to 0.
pub(crate) fn rolling_std(values: &[f64], window: usize) -> Vec<f64> {
    rolling_var(values, window)
        .into_iter()
        .map(|v| if v < 0.0 { 0.0 } else { v.sqrt() })
        .collect()
}

fn rolling_min_max(values: &[f64], window: usize, is_max: bool) -> Vec<f64> {
    let n = values.len();
    let mut output = vec![f64::NAN; n];
    if n == 0 {
        return output;
    }
    // Window 0 → empty windows → all NaN. `_roll_min_max` also raises minp to ≥1.
    if window == 0 {
        return output;
    }
    let values = prep_values(values);
    let minp = window;

    // Indices of monotonic extrema (decreasing values for max, increasing for min).
    let mut candidates: Vec<usize> = Vec::new();

    for i in 0..n {
        let (start, end) = fixed_bounds(i, window);
        debug_assert_eq!(end, i + 1);

        while candidates.first().is_some_and(|&idx| idx < start) {
            candidates.remove(0);
        }

        let val = values[i];
        if ieee_eq(val, val) {
            while let Some(&back) = candidates.last() {
                let back_val = values[back];
                let should_pop = if is_max {
                    val >= back_val
                } else {
                    val <= back_val
                };
                if should_pop {
                    candidates.pop();
                } else {
                    break;
                }
            }
            candidates.push(i);
        }

        let mut nobs = 0usize;
        for &x in &values[start..end] {
            if ieee_eq(x, x) {
                nobs += 1;
            }
        }

        if nobs >= minp {
            if let Some(&front) = candidates.first() {
                output[i] = values[front];
            }
        }
    }

    output
}

/// Pandas `_roll_min_max` with `is_max = true`, FixedWindowIndexer, `min_periods = window`.
pub(crate) fn rolling_max(values: &[f64], window: usize) -> Vec<f64> {
    rolling_min_max(values, window, true)
}

/// Pandas `_roll_min_max` with `is_max = false`, FixedWindowIndexer, `min_periods = window`.
pub(crate) fn rolling_min(values: &[f64], window: usize) -> Vec<f64> {
    rolling_min_max(values, window, false)
}

/// Pandas `aggregations.ewm` with `adjust=False`, `ignore_na=False`, `normalize=True`,
/// over a single full-series window. Does **not** raise `min_periods` to 1 (unlike `roll_var`).
pub(crate) fn ewm_mean(values: &[f64], com: f64, min_periods: usize) -> Vec<f64> {
    let n = values.len();
    let mut output = vec![f64::NAN; n];
    if n == 0 {
        return output;
    }
    let values = prep_values(values);

    let alpha = 1.0 / (1.0 + com);
    let old_wt_factor = 1.0 - alpha;
    // adjust=False → new_wt starts as alpha (may be overwritten when com == 1).
    let mut new_wt = alpha;

    let mut weighted = values[0];
    let mut is_observation = ieee_eq(weighted, weighted);
    let mut nobs: usize = usize::from(is_observation);
    output[0] = if nobs >= min_periods {
        weighted
    } else {
        f64::NAN
    };
    let mut old_wt = 1.0;

    for i in 1..n {
        let cur = values[i];
        is_observation = ieee_eq(cur, cur);
        nobs += usize::from(is_observation);

        if ieee_eq(weighted, weighted) {
            // ignore_na=False ⇒ always enter (decay even on missing).
            old_wt *= old_wt_factor;
            if is_observation {
                // avoid numerical errors on constant series
                if !ieee_eq(weighted, cur) {
                    if ieee_eq(com, 1.0) {
                        new_wt = 1.0 - old_wt;
                    }
                    weighted = ewm_fuse_update(old_wt, weighted, new_wt, cur);
                }
                // adjust=False
                old_wt = 1.0;
            }
        } else if is_observation {
            weighted = cur;
        }

        output[i] = if nobs >= min_periods {
            weighted
        } else {
            f64::NAN
        };
    }

    output
}

/// Unfused `old_wt * weighted + new_wt * cur` then divide by `(old_wt + new_wt)`.
#[expect(
    clippy::suboptimal_flops,
    reason = "pandas evaluates a*b+c unfused; mul_add changes the result bits"
)]
fn ewm_fuse_update(old_wt: f64, weighted: f64, new_wt: f64, cur: f64) -> f64 {
    let mut out = old_wt * weighted + new_wt * cur;
    out /= old_wt + new_wt;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ieee::{com_from_alpha_period, com_from_span};

    fn assert_bits_eq(actual: &[f64], expected: &[f64]) {
        assert_eq!(actual.len(), expected.len(), "length mismatch");
        for (i, (&a, &e)) in actual.iter().zip(expected.iter()).enumerate() {
            if e.is_nan() {
                assert!(a.is_nan(), "index {i}: expected NaN, got {a}");
            } else {
                assert_eq!(
                    a.to_bits(),
                    e.to_bits(),
                    "index {i}: got {a} ({:#x}) expected {e} ({:#x})",
                    a.to_bits(),
                    e.to_bits()
                );
            }
        }
    }

    #[test]
    fn rolling_mean_window2_small_decimals() {
        let got = rolling_mean(&[0.1, 0.2, 0.3, 0.1, 0.1], 2);
        assert_bits_eq(&got, &[f64::NAN, 0.15000000000000002, 0.25, 0.2, 0.1]);
    }

    #[test]
    fn rolling_mean_window3_same_value_shortcut() {
        let got = rolling_mean(&[1e16, 1.0, 1.0, 1.0], 3);
        assert_bits_eq(&got, &[f64::NAN, f64::NAN, 3_333_333_333_333_334.0, 1.0]);
    }

    #[test]
    fn rolling_mean_window2_large_then_small() {
        let got = rolling_mean(&[1e16, 1.0, 2.0, -3.0], 2);
        assert_bits_eq(&got, &[f64::NAN, 5e15, 1.5, -0.5]);
    }

    #[test]
    fn rolling_var_window2_instability_recompute() {
        let got = rolling_var(&[1e15, 1.0, 2.0, 4.0], 2);
        assert_bits_eq(&got, &[f64::NAN, 4.99999999999999e29, 0.5, 2.0]);
    }

    #[test]
    fn rolling_var_window3() {
        let got = rolling_var(&[1e15, 1.0, 2.0, 4.0, 8.0], 3);
        assert_bits_eq(
            &got,
            &[
                f64::NAN,
                f64::NAN,
                3.333333333333323e29,
                2.333333333333333,
                9.333333333333332,
            ],
        );
    }

    #[test]
    fn rolling_std_window2_constant() {
        let got = rolling_std(&[3.0, 3.0, 3.0], 2);
        assert_bits_eq(&got, &[f64::NAN, 0.0, 0.0]);
    }

    #[test]
    fn rolling_std_window3_settles_to_zero() {
        let got = rolling_std(&[0.1, 0.2, 0.3, 0.3, 0.3, 0.3], 3);
        assert_bits_eq(
            &got,
            &[
                f64::NAN,
                f64::NAN,
                0.09999999999999998,
                0.05773502691896253,
                0.0,
                0.0,
            ],
        );
    }

    #[test]
    fn rolling_mean_window0_all_nan() {
        let got = rolling_mean(&[1.0, 2.0, 3.0], 0);
        assert_bits_eq(&got, &[f64::NAN, f64::NAN, f64::NAN]);
    }

    #[test]
    fn rolling_mean_inf_treated_as_nan() {
        let got = rolling_mean(&[1.0, f64::INFINITY, 3.0], 2);
        assert_bits_eq(&got, &[f64::NAN, f64::NAN, f64::NAN]);
    }

    #[test]
    fn rolling_empty_input_empty_output() {
        assert!(rolling_mean(&[], 2).is_empty());
        assert!(rolling_var(&[], 2).is_empty());
        assert!(rolling_std(&[], 2).is_empty());
        assert!(rolling_max(&[], 2).is_empty());
        assert!(rolling_min(&[], 2).is_empty());
    }

    #[test]
    fn rolling_max_window2_with_nan() {
        let got = rolling_max(&[1.0, f64::NAN, 3.0, 2.0], 2);
        assert_bits_eq(&got, &[f64::NAN, f64::NAN, f64::NAN, 3.0]);
    }

    #[test]
    fn donchian_composition_period2() {
        let high = [1.0, 3.0, 2.0, 5.0, 4.0];
        let low = [0.0, 1.0, 1.0, 2.0, 3.0];
        let upper = crate::elementwise::shift(&rolling_max(&high, 2), 1);
        let lower = crate::elementwise::shift(&rolling_min(&low, 2), 1);
        assert_bits_eq(&upper, &[f64::NAN, f64::NAN, 3.0, 3.0, 5.0]);
        assert_bits_eq(&lower, &[f64::NAN, f64::NAN, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn ewm_mean_smma_period2_with_nan() {
        let com = com_from_alpha_period(2);
        let got = ewm_mean(&[1.0, f64::NAN, 3.0, 4.0], com, 0);
        assert_bits_eq(&got, &[1.0, 1.0, 2.5, 3.25]);
    }

    #[test]
    fn ewm_mean_ema_span4_with_nan() {
        let com = com_from_span(4);
        let got = ewm_mean(&[1.0, f64::NAN, 3.0, 4.0], com, 0);
        assert_bits_eq(&got, &[1.0, 1.0, 2.0526315789473686, 2.8315789473684214]);
    }

    #[test]
    fn ewm_mean_span3_equals_smma_period2() {
        let smma = ewm_mean(&[1.0, f64::NAN, 3.0, 4.0], com_from_alpha_period(2), 0);
        let span3 = ewm_mean(&[1.0, f64::NAN, 3.0, 4.0], com_from_span(3), 0);
        assert_bits_eq(&smma, &span3);
    }

    #[test]
    fn ewm_mean_leading_nans_stay_until_first_obs() {
        let got = ewm_mean(&[f64::NAN, f64::NAN, 1.0, 2.0], com_from_span(3), 0);
        assert_bits_eq(&got, &[f64::NAN, f64::NAN, 1.0, 1.5]);
    }

    #[test]
    fn ewm_mean_inf_carried_like_nan() {
        let with_nan = ewm_mean(&[1.0, f64::NAN, 3.0, 4.0], com_from_alpha_period(2), 0);
        let with_inf = ewm_mean(&[1.0, f64::INFINITY, 3.0, 4.0], com_from_alpha_period(2), 0);
        assert_bits_eq(&with_inf, &with_nan);
    }
}
