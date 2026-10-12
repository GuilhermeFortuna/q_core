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

    struct Carried {
        spread: f64,
        session: i64,
        expiry_msc: i64,
    }

    let mut carried: Option<Carried> = None;
    let mut tick_idx = 0usize;

    for bar_i in 0..n_bars {
        let bar_open = bar_open_msc[bar_i];
        let bar_close = bar_close_msc[bar_i];
        let bar_len = bar_close - bar_open;
        if bar_len <= 0 {
            last[bar_i] = carried.as_ref().map(|c| c.spread).unwrap_or(f64::NAN);
            continue;
        }

        let mut t = bar_open;
        let mut weighted_sum = 0.0;
        let mut covered = 0i64;
        let mut max_spread = f64::NAN;

        while t < bar_close {
            let next_expiry = carried.as_ref().map(|c| c.expiry_msc).unwrap_or(i64::MAX);
            let next_tick = if tick_idx < n_ticks {
                time_msc[tick_idx]
            } else {
                i64::MAX
            };
            let next_event = next_expiry.min(next_tick).min(bar_close);

            if let Some(c) = &carried {
                if next_event > t {
                    let dt = next_event - t;
                    let spread = c.spread;
                    #[expect(
                        clippy::suboptimal_flops,
                        reason = "duration weighting must not use fused multiply-add"
                    )]
                    {
                        weighted_sum += spread * (dt as f64);
                    }
                    covered += dt;
                    if dt > 0 && (max_spread.is_nan() || spread > max_spread) {
                        max_spread = spread;
                    }
                }
            }
            t = next_event;

            if next_event >= bar_close {
                break;
            }

            if carried.is_some() && next_event == next_expiry && next_tick != next_expiry {
                carried = None;
                continue;
            }

            if tick_idx < n_ticks && next_event == next_tick {
                let tick_time = time_msc[tick_idx];
                let tick_session = session[tick_idx];
                let tick_flags = flags[tick_idx];
                let is_quote =
                    (tick_flags & MQL5_BID_FLAG != 0) || (tick_flags & MQL5_ASK_FLAG != 0);

                if let Some(c) = &carried {
                    if tick_session != c.session {
                        carried = None;
                    }
                }

                if is_quote {
                    let b = bid[tick_idx];
                    let a = ask[tick_idx];
                    match relative_spread(b, a) {
                        Some(spread) => {
                            carried = Some(Carried {
                                spread,
                                session: tick_session,
                                expiry_msc: tick_time + max_quote_age_ms,
                            });
                        }
                        None => {
                            carried = None;
                        }
                    }
                }
                tick_idx += 1;
            }
        }

        if covered > 0 {
            time_weighted_mean[bar_i] = weighted_sum / (covered as f64);
            max_out[bar_i] = max_spread;
            coverage[bar_i] = (covered as f64) / (bar_len as f64);
        } else {
            time_weighted_mean[bar_i] = f64::NAN;
            max_out[bar_i] = f64::NAN;
            coverage[bar_i] = 0.0;
        }
        last[bar_i] = carried.as_ref().map(|c| c.spread).unwrap_or(f64::NAN);
    }

    Ok(QuoteSpreadBars {
        last,
        time_weighted_mean,
        max: max_out,
        coverage,
    })
}
