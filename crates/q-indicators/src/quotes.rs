//! Quoted bid-ask spread aggregated to chart bars.

use crate::error::IndicatorError;

const MQL5_BID_FLAG: i64 = 2;
const MQL5_ASK_FLAG: i64 = 4;

fn relative_spread(bid: f64, ask: f64) -> Option<f64> {
    if !bid.is_finite() || !ask.is_finite() || bid <= 0.0 || ask < bid {
        return None;
    }
    let mid = (ask + bid) / 2.0;
    Some((ask - bid) / mid)
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

struct Carried {
    spread: f64,
    session: i64,
    expiry_msc: i64,
}

/// Drops a quote whose expiry lies before `now_msc`. A quote adds duration over
/// `[update, expiry)` and is still readable as `last` at the expiry instant itself.
fn drop_expired(carried: &mut Option<Carried>, now_msc: i64) {
    if carried.as_ref().is_some_and(|c| c.expiry_msc < now_msc) {
        *carried = None;
    }
}

fn apply_tick(
    carried: &mut Option<Carried>,
    time_msc: i64,
    bid: f64,
    ask: f64,
    flags: i64,
    session: i64,
    max_quote_age_ms: i64,
) {
    if carried.as_ref().is_some_and(|c| c.session != session) {
        *carried = None;
    }
    if flags & (MQL5_BID_FLAG | MQL5_ASK_FLAG) == 0 {
        return;
    }
    *carried = relative_spread(bid, ask).map(|spread| Carried {
        spread,
        session,
        expiry_msc: time_msc.saturating_add(max_quote_age_ms),
    });
}

/// Bar-aggregated quote spread statistics.
#[derive(Debug, Clone, PartialEq)]
pub struct QuoteSpreadBars {
    pub last: Vec<f64>,
    pub time_weighted_mean: Vec<f64>,
    pub max: Vec<f64>,
    pub coverage: Vec<f64>,
}

/// Aggregate relative quoted spread onto `[open, close)` bars from tick quotes.
#[expect(
    clippy::too_many_arguments,
    reason = "tick and bar columns are explicit per spec"
)]
pub fn quote_spread_bars(
    time_msc: &[i64],
    bid: &[f64],
    ask: &[f64],
    flags: &[i64],
    session: &[i64],
    bar_open_msc: &[i64],
    bar_close_msc: &[i64],
    max_quote_age_ms: i64,
) -> Result<QuoteSpreadBars, IndicatorError> {
    let n_ticks = time_msc.len();
    for (name, len) in [
        ("bid", bid.len()),
        ("ask", ask.len()),
        ("flags", flags.len()),
        ("session", session.len()),
    ] {
        if len != n_ticks {
            return Err(length_mismatch("quote_spread_bars", name, n_ticks, len));
        }
    }
    let n_bars = bar_open_msc.len();
    if bar_close_msc.len() != n_bars {
        return Err(length_mismatch(
            "quote_spread_bars",
            "bar_close_msc",
            n_bars,
            bar_close_msc.len(),
        ));
    }
    if max_quote_age_ms < 0 {
        return Err(IndicatorError::InvalidParameter {
            function: "quote_spread_bars",
            parameter: "max_quote_age_ms",
            value: max_quote_age_ms,
            requirement: ">= 0",
        });
    }

    let mut last = vec![f64::NAN; n_bars];
    let mut time_weighted_mean = vec![f64::NAN; n_bars];
    let mut max_out = vec![f64::NAN; n_bars];
    let mut coverage = vec![0.0; n_bars];

    let mut carried: Option<Carried> = None;
    let mut tick_idx = 0usize;

    for bar_i in 0..n_bars {
        let bar_open = bar_open_msc[bar_i];
        let bar_close = bar_close_msc[bar_i];

        // Ticks before the bar only update the carried quote; they add no duration.
        while tick_idx < n_ticks && time_msc[tick_idx] < bar_open {
            apply_tick(
                &mut carried,
                time_msc[tick_idx],
                bid[tick_idx],
                ask[tick_idx],
                flags[tick_idx],
                session[tick_idx],
                max_quote_age_ms,
            );
            tick_idx += 1;
        }
        drop_expired(&mut carried, bar_open);

        let bar_len = bar_close.saturating_sub(bar_open);
        if bar_len <= 0 {
            last[bar_i] = carried.as_ref().map_or(f64::NAN, |c| c.spread);
            continue;
        }

        let mut t = bar_open;
        let mut weighted_sum = 0.0;
        let mut covered = 0i64;
        let mut max_spread = f64::NAN;

        loop {
            let next_expiry = carried.as_ref().map_or(i64::MAX, |c| c.expiry_msc);
            // A tick is never applied before the cursor, so time only moves forward.
            let next_tick = if tick_idx < n_ticks {
                time_msc[tick_idx].max(t)
            } else {
                i64::MAX
            };
            let next_event = next_expiry.min(next_tick).min(bar_close).max(t);

            if let Some(c) = &carried {
                let dt = next_event - t;
                if dt > 0 {
                    #[expect(
                        clippy::suboptimal_flops,
                        reason = "duration weighting must not use fused multiply-add"
                    )]
                    {
                        weighted_sum += c.spread * (dt as f64);
                    }
                    covered += dt;
                    if max_spread.is_nan() || c.spread > max_spread {
                        max_spread = c.spread;
                    }
                }
            }
            t = next_event;
            if t >= bar_close {
                break;
            }
            if next_expiry <= t {
                carried = None;
            }

            // Equal-time ticks apply in source order; the last one defines the quote.
            while tick_idx < n_ticks && time_msc[tick_idx] <= t {
                apply_tick(
                    &mut carried,
                    t,
                    bid[tick_idx],
                    ask[tick_idx],
                    flags[tick_idx],
                    session[tick_idx],
                    max_quote_age_ms,
                );
                tick_idx += 1;
            }
        }

        if covered > 0 {
            time_weighted_mean[bar_i] = weighted_sum / (covered as f64);
            max_out[bar_i] = max_spread;
            coverage[bar_i] = (covered as f64) / (bar_len as f64);
        }
        drop_expired(&mut carried, bar_close);
        last[bar_i] = carried.as_ref().map_or(f64::NAN, |c| c.spread);
    }

    Ok(QuoteSpreadBars {
        last,
        time_weighted_mean,
        max: max_out,
        coverage,
    })
}
