//! Pandas FixedWindowIndexer rolling primitives (mean, var, std).

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

/// Streaming state for rolling mean over a fixed window.
#[derive(Debug, Clone)]
pub(crate) struct RollingMeanState {
    window: usize,
    min_periods: usize,
    buffer: Vec<f64>,
    head: usize,
    count: usize,
    compensation_add: f64,
    compensation_remove: f64,
    sum_x: f64,
    nobs: usize,
    neg_ct: usize,
    prev_value: f64,
    num_consecutive_same_value: i64,
}

impl RollingMeanState {
    pub(crate) fn new(window: usize, min_periods: usize) -> Self {
        Self {
            window,
            min_periods,
            buffer: if window > 0 {
                vec![f64::NAN; window]
            } else {
                Vec::new()
            },
            head: 0,
            count: 0,
            compensation_add: 0.0,
            compensation_remove: 0.0,
            sum_x: 0.0,
            nobs: 0,
            neg_ct: 0,
            prev_value: 0.0,
            num_consecutive_same_value: 0,
        }
    }

    pub(crate) fn reset(&mut self) {
        if self.window > 0 {
            self.buffer.fill(f64::NAN);
        }
        self.head = 0;
        self.count = 0;
        self.compensation_add = 0.0;
        self.compensation_remove = 0.0;
        self.sum_x = 0.0;
        self.nobs = 0;
        self.neg_ct = 0;
        self.prev_value = 0.0;
        self.num_consecutive_same_value = 0;
    }

    pub(crate) fn push(&mut self, val: f64) -> f64 {
        let val = if window_missing(val) { f64::NAN } else { val };
        if self.window == 0 {
            return f64::NAN;
        }
        if self.window == 1 {
            self.compensation_add = 0.0;
            self.compensation_remove = 0.0;
            self.sum_x = 0.0;
            self.nobs = 0;
            self.neg_ct = 0;
            self.prev_value = val;
            self.num_consecutive_same_value = 0;
            add_mean(
                val,
                &mut self.nobs,
                &mut self.sum_x,
                &mut self.neg_ct,
                &mut self.compensation_add,
                &mut self.num_consecutive_same_value,
                &mut self.prev_value,
            );
            return calc_mean(
                self.min_periods,
                self.nobs,
                self.neg_ct,
                self.sum_x,
                self.num_consecutive_same_value,
                self.prev_value,
            );
        }

        if self.count >= self.window {
            let old_val = self.buffer[self.head];
            remove_mean(
                old_val,
                &mut self.nobs,
                &mut self.sum_x,
                &mut self.neg_ct,
                &mut self.compensation_remove,
            );
        }

        if self.count == 0 {
            self.prev_value = val;
            self.num_consecutive_same_value = 0;
        }

        add_mean(
            val,
            &mut self.nobs,
            &mut self.sum_x,
            &mut self.neg_ct,
            &mut self.compensation_add,
            &mut self.num_consecutive_same_value,
            &mut self.prev_value,
        );

        self.buffer[self.head] = val;
        self.head = (self.head + 1) % self.window;
        self.count = self.count.saturating_add(1);

        calc_mean(
            self.min_periods,
            self.nobs,
            self.neg_ct,
            self.sum_x,
            self.num_consecutive_same_value,
            self.prev_value,
        )
    }
}

/// Pandas `roll_mean` with FixedWindowIndexer bounds and `min_periods = window`.
pub(crate) fn rolling_mean(values: &[f64], window: usize) -> Vec<f64> {
    let mut state = RollingMeanState::new(window, window);
    let mut output = Vec::with_capacity(values.len());
    for &val in values {
        output.push(state.push(val));
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

/// Streaming state for rolling variance over a fixed window.
#[derive(Debug, Clone)]
pub(crate) struct RollingVarState {
    window: usize,
    min_periods: usize,
    ddof: i32,
    buffer: Vec<f64>,
    head: usize,
    count: usize,
    mean_x: f64,
    ssqdm_x: f64,
    nobs: f64,
    compensation_add: f64,
    compensation_remove: f64,
}

impl RollingVarState {
    pub(crate) fn new(window: usize, min_periods: usize, ddof: i32) -> Self {
        Self {
            window,
            min_periods,
            ddof,
            buffer: if window > 0 {
                vec![f64::NAN; window]
            } else {
                Vec::new()
            },
            head: 0,
            count: 0,
            mean_x: 0.0,
            ssqdm_x: 0.0,
            nobs: 0.0,
            compensation_add: 0.0,
            compensation_remove: 0.0,
        }
    }

    pub(crate) fn reset(&mut self) {
        if self.window > 0 {
            self.buffer.fill(f64::NAN);
        }
        self.head = 0;
        self.count = 0;
        self.mean_x = 0.0;
        self.ssqdm_x = 0.0;
        self.nobs = 0.0;
        self.compensation_add = 0.0;
        self.compensation_remove = 0.0;
    }

    pub(crate) fn push(&mut self, val: f64) -> f64 {
        let val = if window_missing(val) { f64::NAN } else { val };
        if self.window == 0 {
            return f64::NAN;
        }

        let mut numerically_unstable = false;

        if self.count >= self.window {
            let old_val = self.buffer[self.head];
            remove_var(
                old_val,
                &mut self.nobs,
                &mut self.mean_x,
                &mut self.ssqdm_x,
                &mut self.compensation_remove,
                &mut numerically_unstable,
            );
        }

        add_var(
            val,
            &mut self.nobs,
            &mut self.mean_x,
            &mut self.ssqdm_x,
            &mut self.compensation_add,
            &mut numerically_unstable,
        );

        self.buffer[self.head] = val;
        self.head = (self.head + 1) % self.window;
        self.count = self.count.saturating_add(1);

        if numerically_unstable {
            self.mean_x = 0.0;
            self.ssqdm_x = 0.0;
            self.nobs = 0.0;
            self.compensation_add = 0.0;
            self.compensation_remove = 0.0;
            let mut dummy_unstable = false;
            let n_items = self.count.min(self.window);
            let start = if self.count < self.window {
                0
            } else {
                self.head
            };
            for k in 0..n_items {
                let item = self.buffer[(start + k) % self.window];
                add_var(
                    item,
                    &mut self.nobs,
                    &mut self.mean_x,
                    &mut self.ssqdm_x,
                    &mut self.compensation_add,
                    &mut dummy_unstable,
                );
            }
        }

        calc_var(self.min_periods, self.ddof, self.nobs, self.ssqdm_x)
    }
}

/// Pandas `roll_var` with FixedWindowIndexer, `ddof = 1`, `min_periods = max(window, 1)`.
pub(crate) fn rolling_var(values: &[f64], window: usize) -> Vec<f64> {
    let mut state = RollingVarState::new(window, window.max(1), 1);
    let mut output = Vec::with_capacity(values.len());
    for &val in values {
        output.push(state.push(val));
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

/// Streaming state for exponential weighted moving average (adjust=False).
#[derive(Debug, Clone)]
pub(crate) struct EwmMeanState {
    com: f64,
    min_periods: usize,
    alpha: f64,
    old_wt_factor: f64,
    new_wt: f64,
    weighted: f64,
    nobs: usize,
    old_wt: f64,
}

impl EwmMeanState {
    pub(crate) fn new(com: f64, min_periods: usize) -> Self {
        let alpha = 1.0 / (1.0 + com);
        let old_wt_factor = 1.0 - alpha;
        let new_wt = alpha;
        Self {
            com,
            min_periods,
            alpha,
            old_wt_factor,
            new_wt,
            weighted: f64::NAN,
            nobs: 0,
            old_wt: 1.0,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.new_wt = self.alpha;
        self.weighted = f64::NAN;
        self.nobs = 0;
        self.old_wt = 1.0;
    }

    pub(crate) fn push(&mut self, val: f64) -> f64 {
        let cur = if window_missing(val) { f64::NAN } else { val };
        let is_observation = ieee_eq(cur, cur);
        self.nobs += usize::from(is_observation);

        if ieee_eq(self.weighted, self.weighted) {
            // ignore_na=False ⇒ always enter (decay even on missing).
            self.old_wt *= self.old_wt_factor;
            if is_observation {
                // avoid numerical errors on constant series
                if !ieee_eq(self.weighted, cur) {
                    if ieee_eq(self.com, 1.0) {
                        self.new_wt = 1.0 - self.old_wt;
                    }
                    self.weighted = ewm_fuse_update(self.old_wt, self.weighted, self.new_wt, cur);
                }
                // adjust=False
                self.old_wt = 1.0;
            }
        } else if is_observation {
            self.weighted = cur;
        }

        if self.nobs >= self.min_periods {
            self.weighted
        } else {
            f64::NAN
        }
    }
}

/// Pandas `aggregations.ewm` with `adjust=False`, `ignore_na=False`, `normalize=True`,
/// over a single full-series window. Does **not** raise `min_periods` to 1 (unlike `roll_var`).
pub(crate) fn ewm_mean(values: &[f64], com: f64, min_periods: usize) -> Vec<f64> {
    let mut state = EwmMeanState::new(com, min_periods);
    let mut output = Vec::with_capacity(values.len());
    for &val in values {
        output.push(state.push(val));
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

/// Sequential linear weighted moving average: `sum(x[k]*(k+1)) / (w*(w+1)/2)`.
///
/// A window is NaN unless `i >= w - 1` and all `w` values are finite after prep.
#[expect(
    clippy::suboptimal_flops,
    reason = "pandas evaluates a*b+c unfused; mul_add changes the result bits"
)]
pub(crate) fn rolling_linear_wma(values: &[f64], window: usize) -> Vec<f64> {
    let n = values.len();
    let mut output = vec![f64::NAN; n];
    if n == 0 || window == 0 {
        return output;
    }
    let values = prep_values(values);
    let denom = (window * (window + 1) / 2) as f64;

    for i in 0..n {
        if i + 1 < window {
            continue;
        }
        let start = i + 1 - window;
        let end = i + 1;
        let mut all_finite = true;
        for &x in &values[start..end] {
            if !ieee_eq(x, x) {
                all_finite = false;
                break;
            }
        }
        if !all_finite {
            continue;
        }
        let mut acc = 0.0;
        for (k, &x) in values[start..end].iter().enumerate() {
            acc += x * ((k + 1) as f64);
        }
        output[i] = acc / denom;
    }

    output
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

    #[test]
    fn rolling_linear_wma_window2_with_inf() {
        let got = rolling_linear_wma(&[1.0, 2.0, f64::INFINITY, 3.0, 4.0], 2);
        assert_bits_eq(
            &got,
            &[
                f64::NAN,
                1.6666666666666667,
                f64::NAN,
                f64::NAN,
                3.6666666666666665,
            ],
        );
    }

    #[test]
    fn rolling_linear_wma_window1_is_identity() {
        let values = [1.5, -2.0, 3.25, 0.0];
        let got = rolling_linear_wma(&values, 1);
        assert_bits_eq(&got, &values);
    }

    #[test]
    fn rolling_linear_wma_window20_arange40_matches_python_sequential() {
        let values: Vec<f64> = (1..=40).map(|x| x as f64).collect();
        let got = rolling_linear_wma(&values, 20);
        // Embedded from a sequential Python sum x[k]*(k+1) / 210 over series 1..=40.
        let expected: [f64; 40] = [
            f64::NAN,
            f64::NAN,
            f64::NAN,
            f64::NAN,
            f64::NAN,
            f64::NAN,
            f64::NAN,
            f64::NAN,
            f64::NAN,
            f64::NAN,
            f64::NAN,
            f64::NAN,
            f64::NAN,
            f64::NAN,
            f64::NAN,
            f64::NAN,
            f64::NAN,
            f64::NAN,
            f64::NAN,
            f64::from_bits(0x402b_5555_5555_5555),
            f64::from_bits(0x402d_5555_5555_5555),
            f64::from_bits(0x402f_5555_5555_5555),
            f64::from_bits(0x4030_aaaa_aaaa_aaab),
            f64::from_bits(0x4031_aaaa_aaaa_aaab),
            f64::from_bits(0x4032_aaaa_aaaa_aaab),
            f64::from_bits(0x4033_aaaa_aaaa_aaab),
            f64::from_bits(0x4034_aaaa_aaaa_aaab),
            f64::from_bits(0x4035_aaaa_aaaa_aaab),
            f64::from_bits(0x4036_aaaa_aaaa_aaab),
            f64::from_bits(0x4037_aaaa_aaaa_aaab),
            f64::from_bits(0x4038_aaaa_aaaa_aaab),
            f64::from_bits(0x4039_aaaa_aaaa_aaab),
            f64::from_bits(0x403a_aaaa_aaaa_aaab),
            f64::from_bits(0x403b_aaaa_aaaa_aaab),
            f64::from_bits(0x403c_aaaa_aaaa_aaab),
            f64::from_bits(0x403d_aaaa_aaaa_aaab),
            f64::from_bits(0x403e_aaaa_aaaa_aaab),
            f64::from_bits(0x403f_aaaa_aaaa_aaab),
            f64::from_bits(0x4040_5555_5555_5555),
            f64::from_bits(0x4040_d555_5555_5555),
        ];
        assert_bits_eq(&got, &expected);
    }
}
