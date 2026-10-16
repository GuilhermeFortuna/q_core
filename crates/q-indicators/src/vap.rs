//! Volume-at-price profiles from trade tapes.

use std::collections::BTreeMap;

use crate::error::IndicatorError;
use crate::volume::{AggressorSide, BUY_FLAG, SELL_FLAG};

const OFF_GRID_EPS: f64 = 1e-6;

#[derive(Debug, Clone, Default, PartialEq)]
struct RowAccum {
    total: f64,
    buy: f64,
    sell: f64,
    unknown: f64,
    trades: i64,
}

impl RowAccum {
    fn total(&self) -> f64 {
        self.total
    }

    fn add(&mut self, volume: f64, side: AggressorSide) {
        self.total += volume;
        self.trades += 1;
        match side {
            AggressorSide::Buy => self.buy += volume,
            AggressorSide::Sell => self.sell += volume,
            AggressorSide::Unknown => self.unknown += volume,
        }
    }

    fn delta(&self) -> f64 {
        self.buy - self.sell
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Profile {
    rows: BTreeMap<i64, RowAccum>,
    totals: RowAccum,
    skipped_trades: i64,
    off_grid_trades: i64,
}

impl Profile {
    fn clear(&mut self) {
        self.rows.clear();
        self.totals = RowAccum::default();
        self.skipped_trades = 0;
        self.off_grid_trades = 0;
    }

    fn is_trade(flags: i64) -> bool {
        let raw = flags as u32;
        raw & BUY_FLAG != 0 || raw & SELL_FLAG != 0
    }

    fn add_trade(
        &mut self,
        price: f64,
        volume: f64,
        flags: i64,
        tick_size: f64,
        row_ticks: i64,
        function: &'static str,
    ) -> Result<(), IndicatorError> {
        if !price.is_finite() || price <= 0.0 || !volume.is_finite() || volume <= 0.0 {
            self.skipped_trades += 1;
            return Ok(());
        }
        let ratio = price / tick_size;
        // i64::MAX rounds up to 2^63 as f64; the upper bound must be exclusive.
        if !ratio.is_finite() || ratio.round_ties_even() >= i64::MAX as f64 {
            return Err(IndicatorError::InvalidInput {
                function,
                parameter: "price",
                value: price,
                requirement: "price / tick_size must round to a representable int64 tick index",
            });
        }
        let k = round_half_even(ratio);
        if (ratio - k as f64).abs() > OFF_GRID_EPS {
            self.off_grid_trades += 1;
        }
        let row = floor_div(k, row_ticks);
        let side = AggressorSide::from_flags(flags as u32);
        let entry = self.rows.entry(row).or_default();
        entry.add(volume, side);
        self.totals.add(volume, side);
        Ok(())
    }
}

fn row_price(row: i64, row_ticks: i64, tick_size: f64) -> f64 {
    (row * row_ticks) as f64 * tick_size
}

fn round_half_even(x: f64) -> i64 {
    x.round_ties_even() as i64
}

fn floor_div(n: i64, d: i64) -> i64 {
    if n >= 0 {
        n / d
    } else {
        (n - d + 1) / d
    }
}

struct Levels {
    poc: f64,
    vah: f64,
    val: f64,
}

fn poc_and_value_area(
    rows: &BTreeMap<i64, RowAccum>,
    tick_size: f64,
    row_ticks: i64,
    value_area: f64,
    total_vol: f64,
) -> Levels {
    if rows.is_empty() {
        return Levels {
            poc: f64::NAN,
            vah: f64::NAN,
            val: f64::NAN,
        };
    }
    let min_r = rows.keys().next().copied().unwrap();
    let max_r = rows.keys().next_back().copied().unwrap();
    let centre_twice = i128::from(min_r) + i128::from(max_r);

    let mut poc_row = min_r;
    let mut poc_total = -1.0;
    for (&r, acc) in rows.iter() {
        let t = acc.total();
        let dist_r = dist_row(r, centre_twice);
        let dist_poc = dist_row(poc_row, centre_twice);
        let better = t > poc_total
            || (t.to_bits() == poc_total.to_bits()
                && (dist_r < dist_poc || (dist_r == dist_poc && r < poc_row)));
        if better {
            poc_total = t;
            poc_row = r;
        }
    }

    if value_area.to_bits() == 1.0_f64.to_bits() {
        return Levels {
            poc: row_price(poc_row, row_ticks, tick_size),
            vah: row_price(max_r, row_ticks, tick_size),
            val: row_price(min_r, row_ticks, tick_size),
        };
    }
    let target = value_area * total_vol;
    let mut included: BTreeMap<i64, f64> = BTreeMap::new();
    included.insert(poc_row, rows.get(&poc_row).map_or(0.0, RowAccum::total));
    let mut va_vol = included[&poc_row];

    let mut low = poc_row;
    let mut high = poc_row;
    while va_vol < target && (low > min_r || high < max_r) {
        let below = if low > min_r {
            Some((low - 1, row_volume(rows, low - 1)))
        } else {
            None
        };
        let above = if high < max_r {
            Some((high + 1, row_volume(rows, high + 1)))
        } else {
            None
        };
        match (below, above) {
            (Some((rb, vb)), Some((_ra, va))) if vb > va => {
                low = rb;
                va_vol += vb;
                included.insert(rb, vb);
            }
            (Some((_rb, vb)), Some((ra, va))) if va > vb => {
                high = ra;
                va_vol += va;
                included.insert(ra, va);
            }
            (Some((rb, vb)), Some((ra, va))) => {
                low = rb;
                high = ra;
                va_vol += vb + va;
                included.insert(rb, vb);
                included.insert(ra, va);
            }
            (Some((rb, vb)), None) => {
                low = rb;
                va_vol += vb;
                included.insert(rb, vb);
            }
            (None, Some((ra, va))) => {
                high = ra;
                va_vol += va;
                included.insert(ra, va);
            }
            (None, None) => break,
        }
    }

    let val_row = included.keys().next().copied().unwrap();
    let vah_row = included.keys().next_back().copied().unwrap();
    Levels {
        poc: row_price(poc_row, row_ticks, tick_size),
        vah: row_price(vah_row, row_ticks, tick_size),
        val: row_price(val_row, row_ticks, tick_size),
    }
}

fn dist_row(r: i64, centre_twice: i128) -> i128 {
    (2 * i128::from(r) - centre_twice).abs()
}

fn row_volume(rows: &BTreeMap<i64, RowAccum>, r: i64) -> f64 {
    rows.get(&r).map_or(0.0, RowAccum::total)
}

fn length_mismatch(
    function: &'static str,
    parameter: &'static str,
    expected: usize,
    actual: usize,
) -> IndicatorError {
    IndicatorError::LengthMismatch {
        function,
        parameter,
        expected,
        actual,
    }
}

fn validate_tick_size(function: &'static str, tick_size: f64) -> Result<(), IndicatorError> {
    if !tick_size.is_finite() || tick_size <= 0.0 {
        return Err(IndicatorError::InvalidInput {
            function,
            parameter: "tick_size",
            value: tick_size,
            requirement: "finite and > 0",
        });
    }
    Ok(())
}

fn validate_value_area(function: &'static str, value_area: f64) -> Result<(), IndicatorError> {
    if !value_area.is_finite() || value_area <= 0.0 || value_area > 1.0 {
        return Err(IndicatorError::InvalidInput {
            function,
            parameter: "value_area",
            value: value_area,
            requirement: "finite and in (0, 1]",
        });
    }
    Ok(())
}

fn validate_row_ticks(function: &'static str, row_ticks: i64) -> Result<(), IndicatorError> {
    if row_ticks < 1 {
        return Err(IndicatorError::InvalidParameter {
            function,
            parameter: "row_ticks",
            value: row_ticks,
            requirement: ">= 1",
        });
    }
    Ok(())
}

fn validate_non_decreasing_times(
    function: &'static str,
    parameter: &'static str,
    times: &[i64],
) -> Result<(), IndicatorError> {
    for i in 1..times.len() {
        if times[i] < times[i - 1] {
            return Err(IndicatorError::InvalidParameter {
                function,
                parameter,
                value: times[i],
                requirement: "non-decreasing",
            });
        }
    }
    Ok(())
}

fn validate_segments(
    function: &'static str,
    open: &[i64],
    close: &[i64],
) -> Result<(), IndicatorError> {
    let n = open.len();
    for i in 0..n {
        if close[i] <= open[i] {
            return Err(IndicatorError::InvalidParameter {
                function,
                parameter: "segment_close_msc",
                value: close[i],
                requirement: "> segment_open_msc",
            });
        }
        if i > 0 && open[i] < open[i - 1] {
            return Err(IndicatorError::InvalidParameter {
                function,
                parameter: "segment_open_msc",
                value: open[i],
                requirement: "non-decreasing segment opens",
            });
        }
        if i > 0 && open[i] < close[i - 1] {
            return Err(IndicatorError::InvalidParameter {
                function,
                parameter: "segment_open_msc",
                value: open[i],
                requirement: "non-overlapping segments",
            });
        }
    }
    Ok(())
}

/// Per-row volume-at-price output columns.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeAtPriceRows {
    pub segment: Vec<i64>,
    pub price: Vec<f64>,
    pub buy: Vec<f64>,
    pub sell: Vec<f64>,
    pub unknown: Vec<f64>,
    pub total: Vec<f64>,
    pub delta: Vec<f64>,
    pub trades: Vec<i64>,
}

/// Per-segment summary columns.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeAtPriceSegments {
    pub total: Vec<f64>,
    pub buy: Vec<f64>,
    pub sell: Vec<f64>,
    pub unknown: Vec<f64>,
    pub trades: Vec<i64>,
    pub poc: Vec<f64>,
    pub vah: Vec<f64>,
    pub val: Vec<f64>,
    pub skipped_trades: Vec<i64>,
    pub off_grid_trades: Vec<i64>,
}

/// Combined `volume_at_price` result.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeAtPriceOutput {
    pub rows: VolumeAtPriceRows,
    pub segments: VolumeAtPriceSegments,
}

fn finalize_segment(
    profile: &Profile,
    segment_index: i64,
    tick_size: f64,
    row_ticks: i64,
    value_area: f64,
    rows_out: &mut VolumeAtPriceRows,
    segments_out: &mut VolumeAtPriceSegments,
) {
    let RowAccum {
        total,
        buy,
        sell,
        unknown,
        trades,
    } = profile.totals;
    let levels = poc_and_value_area(
        &profile.rows,
        tick_size,
        row_ticks,
        value_area,
        profile.totals.total,
    );
    for (&r, acc) in profile.rows.iter() {
        rows_out.segment.push(segment_index);
        rows_out.price.push(row_price(r, row_ticks, tick_size));
        rows_out.buy.push(acc.buy);
        rows_out.sell.push(acc.sell);
        rows_out.unknown.push(acc.unknown);
        rows_out.total.push(acc.total());
        rows_out.delta.push(acc.delta());
        rows_out.trades.push(acc.trades);
    }
    segments_out.total.push(total);
    segments_out.buy.push(buy);
    segments_out.sell.push(sell);
    segments_out.unknown.push(unknown);
    segments_out.trades.push(trades);
    segments_out.poc.push(levels.poc);
    segments_out.vah.push(levels.vah);
    segments_out.val.push(levels.val);
    segments_out.skipped_trades.push(profile.skipped_trades);
    segments_out.off_grid_trades.push(profile.off_grid_trades);
}

/// Volume at price for caller-defined `[open, close)` segments.
#[expect(
    clippy::too_many_arguments,
    reason = "tape and segment columns are explicit per spec"
)]
pub fn volume_at_price(
    time_msc: &[i64],
    price: &[f64],
    volume: &[f64],
    flags: &[i64],
    segment_open_msc: &[i64],
    segment_close_msc: &[i64],
    tick_size: f64,
    row_ticks: i64,
    value_area: f64,
) -> Result<VolumeAtPriceOutput, IndicatorError> {
    const FN: &str = "volume_at_price";
    let n_ticks = time_msc.len();
    for (name, len) in [
        ("price", price.len()),
        ("volume", volume.len()),
        ("flags", flags.len()),
    ] {
        if len != n_ticks {
            return Err(length_mismatch(FN, name, n_ticks, len));
        }
    }
    if segment_close_msc.len() != segment_open_msc.len() {
        return Err(length_mismatch(
            FN,
            "segment_close_msc",
            segment_open_msc.len(),
            segment_close_msc.len(),
        ));
    }
    validate_tick_size(FN, tick_size)?;
    validate_row_ticks(FN, row_ticks)?;
    validate_value_area(FN, value_area)?;
    validate_non_decreasing_times(FN, "time_msc", time_msc)?;
    validate_segments(FN, segment_open_msc, segment_close_msc)?;

    let mut rows_out = VolumeAtPriceRows {
        segment: Vec::new(),
        price: Vec::new(),
        buy: Vec::new(),
        sell: Vec::new(),
        unknown: Vec::new(),
        total: Vec::new(),
        delta: Vec::new(),
        trades: Vec::new(),
    };
    let mut segments_out = VolumeAtPriceSegments {
        total: Vec::new(),
        buy: Vec::new(),
        sell: Vec::new(),
        unknown: Vec::new(),
        trades: Vec::new(),
        poc: Vec::new(),
        vah: Vec::new(),
        val: Vec::new(),
        skipped_trades: Vec::new(),
        off_grid_trades: Vec::new(),
    };

    let mut profile = Profile::default();
    let mut tick_idx = 0usize;
    for (seg_i, (&open, &close)) in segment_open_msc
        .iter()
        .zip(segment_close_msc.iter())
        .enumerate()
    {
        profile.clear();
        while tick_idx < n_ticks && time_msc[tick_idx] < open {
            tick_idx += 1;
        }
        while tick_idx < n_ticks && time_msc[tick_idx] < close {
            let f = flags[tick_idx];
            if Profile::is_trade(f) {
                profile.add_trade(
                    price[tick_idx],
                    volume[tick_idx],
                    f,
                    tick_size,
                    row_ticks,
                    FN,
                )?;
            }
            tick_idx += 1;
        }
        finalize_segment(
            &profile,
            seg_i as i64,
            tick_size,
            row_ticks,
            value_area,
            &mut rows_out,
            &mut segments_out,
        );
    }

    Ok(VolumeAtPriceOutput {
        rows: rows_out,
        segments: segments_out,
    })
}

/// Developing volume-at-price levels at checkpoints (strictly before each checkpoint time).
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeAtPriceLevels {
    pub poc: Vec<f64>,
    pub vah: Vec<f64>,
    pub val: Vec<f64>,
    pub total: Vec<f64>,
}

fn emit_checkpoint(
    profile: &Profile,
    checkpoint_session: i64,
    current_session: Option<i64>,
    tick_size: f64,
    row_ticks: i64,
    value_area: f64,
    out: &mut VolumeAtPriceLevels,
) {
    if current_session != Some(checkpoint_session) || profile.rows.is_empty() {
        out.poc.push(f64::NAN);
        out.vah.push(f64::NAN);
        out.val.push(f64::NAN);
        out.total.push(0.0);
        return;
    }
    let total = profile.totals.total;
    let levels = poc_and_value_area(
        &profile.rows,
        tick_size,
        row_ticks,
        value_area,
        profile.totals.total,
    );
    out.poc.push(levels.poc);
    out.vah.push(levels.vah);
    out.val.push(levels.val);
    out.total.push(total);
}

#[expect(
    clippy::too_many_arguments,
    reason = "tape and checkpoint columns are explicit per spec"
)]
pub fn volume_at_price_levels(
    time_msc: &[i64],
    price: &[f64],
    volume: &[f64],
    flags: &[i64],
    session: &[i64],
    checkpoint_msc: &[i64],
    checkpoint_session: &[i64],
    tick_size: f64,
    row_ticks: i64,
    value_area: f64,
) -> Result<VolumeAtPriceLevels, IndicatorError> {
    const FN: &str = "volume_at_price_levels";
    let n_ticks = time_msc.len();
    for (name, len) in [
        ("price", price.len()),
        ("volume", volume.len()),
        ("flags", flags.len()),
        ("session", session.len()),
    ] {
        if len != n_ticks {
            return Err(length_mismatch(FN, name, n_ticks, len));
        }
    }
    if checkpoint_session.len() != checkpoint_msc.len() {
        return Err(length_mismatch(
            FN,
            "checkpoint_session",
            checkpoint_msc.len(),
            checkpoint_session.len(),
        ));
    }
    validate_tick_size(FN, tick_size)?;
    validate_row_ticks(FN, row_ticks)?;
    validate_value_area(FN, value_area)?;
    validate_non_decreasing_times(FN, "time_msc", time_msc)?;
    validate_non_decreasing_times(FN, "checkpoint_msc", checkpoint_msc)?;

    let mut out = VolumeAtPriceLevels {
        poc: Vec::with_capacity(checkpoint_msc.len()),
        vah: Vec::with_capacity(checkpoint_msc.len()),
        val: Vec::with_capacity(checkpoint_msc.len()),
        total: Vec::with_capacity(checkpoint_msc.len()),
    };

    let mut profile = Profile::default();
    let mut current_session: Option<i64> = None;
    let mut tick_idx = 0usize;

    for (cp_i, &cp) in checkpoint_msc.iter().enumerate() {
        while tick_idx < n_ticks && time_msc[tick_idx] < cp {
            let sess = session[tick_idx];
            if current_session != Some(sess) {
                profile.clear();
                current_session = Some(sess);
            }
            let f = flags[tick_idx];
            if Profile::is_trade(f) {
                profile.add_trade(
                    price[tick_idx],
                    volume[tick_idx],
                    f,
                    tick_size,
                    row_ticks,
                    FN,
                )?;
            }
            tick_idx += 1;
        }
        emit_checkpoint(
            &profile,
            checkpoint_session[cp_i],
            current_session,
            tick_size,
            row_ticks,
            value_area,
            &mut out,
        );
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_half_even_matches_spec() {
        assert_eq!(round_half_even(200.0), 200);
        assert_eq!(round_half_even(200.5), 200);
        assert_eq!(round_half_even(201.5), 202);
    }

    #[test]
    fn floor_div_negative_tick_index() {
        assert_eq!(floor_div(-1, 2), -1);
        assert_eq!(floor_div(-2, 2), -1);
        assert_eq!(floor_div(-3, 2), -2);
    }
}
