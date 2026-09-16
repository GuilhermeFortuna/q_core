//! Single-position tick simulation (port of `_simulate_njit` / `_compute_quantity`).

/// Position sizing for the tick kernel.
#[derive(Clone, Copy, Debug)]
pub enum TickSizing {
    /// Used as given; never floored.
    FixedQuantity { quantity: f64 },
    /// `max_contracts == 0` means no maximum. Truncates toward zero like numba `int()`.
    FixedSafetyMargin {
        margin_per_contract: f64,
        min_contracts: i64,
        max_contracts: i64,
    },
}

/// Exit reason codes matching the Python `ExitReason` ints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickExitReason {
    StopLoss = 1,
    TakeProfit = 2,
    Signal = 3,
    EndOfDay = 4,
}

/// Column views for one tick stream; every slice must share `bid`'s length.
pub struct TickInputs<'a> {
    pub bid: &'a [f64],
    pub ask: &'a [f64],
    /// Sign only (`+1` / `-1` / `0`).
    pub direction: &'a [i8],
    /// `NaN` = no stop on that entry tick.
    pub sl_points: &'a [f64],
    /// `NaN` = no target on that entry tick.
    pub tp_points: &'a [f64],
}

/// Exact-length trade ledger (one row per closed trade).
#[derive(Clone, Debug, Default)]
pub struct TickLedger {
    pub entry_idx: Vec<i64>,
    pub exit_idx: Vec<i64>,
    pub entry_price: Vec<f64>,
    pub exit_price: Vec<f64>,
    pub direction: Vec<i8>,
    pub quantity: Vec<f64>,
    /// [`TickExitReason`] codes.
    pub exit_reason: Vec<i64>,
}

/// Simulation result: closed trades plus ending capital.
#[derive(Clone, Debug)]
pub struct TickRun {
    pub trades: TickLedger,
    pub final_capital: f64,
}

/// Errors from tick simulation and bar aggregation.
#[derive(Clone, Debug, PartialEq)]
pub enum TickError {
    LengthMismatch {
        column: &'static str,
        expected: usize,
        actual: usize,
    },
    InvalidSizing {
        field: &'static str,
        reason: &'static str,
    },
    /// `int()` of a NaN or infinite volume sum raises today.
    VolumeNotFinite { bar: usize },
}

/// Single-position intrabar tick simulation.
pub fn simulate_ticks(
    inputs: &TickInputs<'_>,
    initial_capital: f64,
    point_value: f64,
    sizing: TickSizing,
) -> Result<TickRun, TickError> {
    let n = inputs.bid.len();
    check_len("ask", n, inputs.ask.len())?;
    check_len("direction", n, inputs.direction.len())?;
    check_len("sl_points", n, inputs.sl_points.len())?;
    check_len("tp_points", n, inputs.tp_points.len())?;

    let mut trades = TickLedger::default();
    let mut capital = initial_capital;
    let mut position_dir: i8 = 0;
    let mut pos_entry_idx: i64 = 0;
    let mut pos_entry_price = 0.0;
    let mut pos_quantity = 0.0;
    let mut sl_price = 0.0;
    let mut tp_price = 0.0;
    let mut has_sl = false;
    let mut has_tp = false;
    let mut block_reentry = false;

    for i in 0..n {
        if block_reentry {
            block_reentry = false;
        }

        if position_dir == 0 {
            let sig = inputs.direction[i];
            if sig != 0 {
                let qty = compute_quantity(capital, sizing);
                if qty > 0.0 {
                    if sig > 0 {
                        position_dir = 1;
                        pos_entry_price = inputs.ask[i];
                    } else {
                        position_dir = -1;
                        pos_entry_price = inputs.bid[i];
                    }

                    pos_entry_idx = i as i64;
                    pos_quantity = qty;

                    let sl_pts = inputs.sl_points[i];
                    let tp_pts = inputs.tp_points[i];
                    has_sl = !sl_pts.is_nan();
                    has_tp = !tp_pts.is_nan();

                    if has_sl {
                        if position_dir > 0 {
                            sl_price = long_stop_level(pos_entry_price, sl_pts);
                        } else {
                            sl_price = short_stop_level(pos_entry_price, sl_pts);
                        }
                    }
                    if has_tp {
                        if position_dir > 0 {
                            tp_price = long_target_level(pos_entry_price, tp_pts);
                        } else {
                            tp_price = short_target_level(pos_entry_price, tp_pts);
                        }
                    }
                }
            }
        } else {
            let mut exited = false;
            let mut reason = 0_i64;
            let fill_price;

            if position_dir > 0 {
                fill_price = inputs.bid[i];
                if has_sl && inputs.bid[i] <= sl_price {
                    reason = TickExitReason::StopLoss as i64;
                    exited = true;
                } else if has_tp && inputs.bid[i] >= tp_price {
                    reason = TickExitReason::TakeProfit as i64;
                    exited = true;
                } else if inputs.direction[i] < 0 {
                    reason = TickExitReason::Signal as i64;
                    exited = true;
                }
            } else {
                fill_price = inputs.ask[i];
                if has_sl && inputs.ask[i] >= sl_price {
                    reason = TickExitReason::StopLoss as i64;
                    exited = true;
                } else if has_tp && inputs.ask[i] <= tp_price {
                    reason = TickExitReason::TakeProfit as i64;
                    exited = true;
                } else if inputs.direction[i] > 0 {
                    reason = TickExitReason::Signal as i64;
                    exited = true;
                }
            }

            if exited {
                let pnl = if position_dir > 0 {
                    long_pnl(fill_price, pos_entry_price, pos_quantity, point_value)
                } else {
                    short_pnl(pos_entry_price, fill_price, pos_quantity, point_value)
                };
                capital += pnl;
                trades.entry_idx.push(pos_entry_idx);
                trades.exit_idx.push(i as i64);
                trades.entry_price.push(pos_entry_price);
                trades.exit_price.push(fill_price);
                trades.direction.push(position_dir);
                trades.quantity.push(pos_quantity);
                trades.exit_reason.push(reason);
                position_dir = 0;
                block_reentry = true;
            }
        }
    }

    if position_dir != 0 {
        let (fill_price, pnl) = if position_dir > 0 {
            let fill = inputs.bid[n - 1];
            (
                fill,
                long_pnl(fill, pos_entry_price, pos_quantity, point_value),
            )
        } else {
            let fill = inputs.ask[n - 1];
            (
                fill,
                short_pnl(pos_entry_price, fill, pos_quantity, point_value),
            )
        };
        capital += pnl;
        trades.entry_idx.push(pos_entry_idx);
        trades.exit_idx.push((n - 1) as i64);
        trades.entry_price.push(pos_entry_price);
        trades.exit_price.push(fill_price);
        trades.direction.push(position_dir);
        trades.quantity.push(pos_quantity);
        trades.exit_reason.push(TickExitReason::EndOfDay as i64);
    }

    Ok(TickRun {
        trades,
        final_capital: capital,
    })
}

fn check_len(column: &'static str, expected: usize, actual: usize) -> Result<(), TickError> {
    if actual == expected {
        Ok(())
    } else {
        Err(TickError::LengthMismatch {
            column,
            expected,
            actual,
        })
    }
}

fn compute_quantity(capital: f64, sizing: TickSizing) -> f64 {
    match sizing {
        TickSizing::FixedQuantity { quantity } => quantity,
        TickSizing::FixedSafetyMargin {
            margin_per_contract,
            min_contracts,
            max_contracts,
        } => {
            // numba `int(capital / sizing_a)` truncates toward zero.
            let mut qty = (capital / margin_per_contract).trunc() as i64;
            if max_contracts > 0 && qty > max_contracts {
                qty = max_contracts;
            }
            let min_c = min_contracts;
            if qty < min_c {
                if min_c > 0 && qty == 0 {
                    qty = min_c;
                } else {
                    return 0.0;
                }
            }
            if qty <= 0 {
                return 0.0;
            }
            qty as f64
        }
    }
}

#[inline]
#[allow(
    clippy::suboptimal_flops,
    reason = "exact parity with numba: entry - sl_pts must not fuse"
)]
fn long_stop_level(entry: f64, sl_pts: f64) -> f64 {
    entry - sl_pts
}

#[inline]
#[allow(
    clippy::suboptimal_flops,
    reason = "exact parity with numba: entry + sl_pts must not fuse"
)]
fn short_stop_level(entry: f64, sl_pts: f64) -> f64 {
    entry + sl_pts
}

#[inline]
#[allow(
    clippy::suboptimal_flops,
    reason = "exact parity with numba: entry + tp_pts must not fuse"
)]
fn long_target_level(entry: f64, tp_pts: f64) -> f64 {
    entry + tp_pts
}

#[inline]
#[allow(
    clippy::suboptimal_flops,
    reason = "exact parity with numba: entry - tp_pts must not fuse"
)]
fn short_target_level(entry: f64, tp_pts: f64) -> f64 {
    entry - tp_pts
}

#[inline]
#[allow(
    clippy::suboptimal_flops,
    reason = "exact parity with numba: ((fill-entry)*qty)*pv left-assoc, no mul-add fuse"
)]
fn long_pnl(fill: f64, entry: f64, qty: f64, point_value: f64) -> f64 {
    ((fill - entry) * qty) * point_value
}

#[inline]
#[allow(
    clippy::suboptimal_flops,
    reason = "exact parity with numba: ((entry-fill)*qty)*pv left-assoc, no mul-add fuse"
)]
fn short_pnl(entry: f64, fill: f64, qty: f64, point_value: f64) -> f64 {
    ((entry - fill) * qty) * point_value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs<'a>(
        bid: &'a [f64],
        ask: &'a [f64],
        direction: &'a [i8],
        sl_points: &'a [f64],
        tp_points: &'a [f64],
    ) -> TickInputs<'a> {
        TickInputs {
            bid,
            ask,
            direction,
            sl_points,
            tp_points,
        }
    }

    #[test]
    fn long_touch_stop_exits_at_exact_level() {
        let bid = [100.0, 99.5];
        let ask = [100.0, 99.5];
        let direction = [1_i8, 0];
        let sl = [0.5, 0.5];
        let tp = [f64::NAN, f64::NAN];
        let run = simulate_ticks(
            &inputs(&bid, &ask, &direction, &sl, &tp),
            100_000.0,
            1.0,
            TickSizing::FixedQuantity { quantity: 1.0 },
        )
        .unwrap();
        assert_eq!(
            run.trades.exit_reason,
            vec![TickExitReason::StopLoss as i64]
        );
        assert_eq!(run.trades.exit_price, vec![99.5]);
        assert_eq!(run.trades.exit_idx, vec![1]);
        assert_eq!(run.trades.entry_price, vec![100.0]);
    }

    #[test]
    fn stop_checked_before_take_profit_on_same_tick() {
        // Bid gaps through both SL (98) and would also be past any lower level;
        // with SL 2 and TP 3 from 100, tick at 97 hits SL; later 104 would be TP.
        let bid = [100.0, 97.0, 104.0];
        let ask = [100.0, 97.0, 104.0];
        let direction = [1_i8, 0, 0];
        let sl = [2.0, 2.0, 2.0];
        let tp = [3.0, 3.0, 3.0];
        let run = simulate_ticks(
            &inputs(&bid, &ask, &direction, &sl, &tp),
            100_000.0,
            1.0,
            TickSizing::FixedQuantity { quantity: 1.0 },
        )
        .unwrap();
        assert_eq!(
            run.trades.exit_reason,
            vec![TickExitReason::StopLoss as i64]
        );
        assert_eq!(run.trades.exit_idx, vec![1]);
        assert_eq!(run.trades.exit_price, vec![97.0]);
    }

    #[test]
    fn opposite_signal_exits_without_reentry_same_tick() {
        let bid = [50.0, 50.0, 50.0, 50.0];
        let ask = [50.0, 50.0, 50.0, 50.0];
        let direction = [1_i8, 0, -1, 1];
        let nan = [f64::NAN; 4];
        let run = simulate_ticks(
            &inputs(&bid, &ask, &direction, &nan, &nan),
            100_000.0,
            1.0,
            TickSizing::FixedQuantity { quantity: 1.0 },
        )
        .unwrap();
        assert_eq!(run.trades.exit_reason.len(), 2);
        assert_eq!(run.trades.exit_reason[0], TickExitReason::Signal as i64);
        assert_eq!(run.trades.exit_idx[0], 2);
        assert_eq!(run.trades.direction[0], 1);
        // No short opened on the exit tick; second trade is long on tick 3.
        assert_eq!(run.trades.entry_idx[1], 3);
        assert_eq!(run.trades.direction[1], 1);
        assert_eq!(run.trades.exit_reason[1], TickExitReason::EndOfDay as i64);
    }

    #[test]
    fn nan_stop_on_entry_ignores_later_finite_stop() {
        let bid = [100.0, 99.0, 98.0];
        let ask = [100.0, 99.0, 98.0];
        let direction = [1_i8, 0, 0];
        let sl = [f64::NAN, 0.5, 0.5];
        let tp = [f64::NAN; 3];
        let run = simulate_ticks(
            &inputs(&bid, &ask, &direction, &sl, &tp),
            100_000.0,
            1.0,
            TickSizing::FixedQuantity { quantity: 1.0 },
        )
        .unwrap();
        assert_eq!(
            run.trades.exit_reason,
            vec![TickExitReason::EndOfDay as i64]
        );
        assert_eq!(run.trades.exit_price, vec![98.0]);
    }

    #[test]
    fn open_position_at_end_closes_at_last_bid_end_of_day() {
        let bid = [10.0, 11.0];
        let ask = [10.0, 11.0];
        let direction = [1_i8, 0];
        let nan = [f64::NAN; 2];
        let run = simulate_ticks(
            &inputs(&bid, &ask, &direction, &nan, &nan),
            100_000.0,
            1.0,
            TickSizing::FixedQuantity { quantity: 1.0 },
        )
        .unwrap();
        assert_eq!(
            run.trades.exit_reason,
            vec![TickExitReason::EndOfDay as i64]
        );
        assert_eq!(run.trades.exit_idx, vec![1]);
        assert_eq!(run.trades.exit_price, vec![11.0]);
    }

    #[test]
    fn fixed_quantity_fractional_never_floored() {
        let bid = [100.0, 100.0];
        let ask = [100.0, 100.0];
        let direction = [1_i8, -1];
        let nan = [f64::NAN; 2];
        let run = simulate_ticks(
            &inputs(&bid, &ask, &direction, &nan, &nan),
            100_000.0,
            1.0,
            TickSizing::FixedQuantity { quantity: 1.5 },
        )
        .unwrap();
        assert_eq!(run.trades.quantity, vec![1.5]);
    }

    #[test]
    fn safety_margin_max_zero_is_none_and_truncates() {
        let bid = [100.0, 100.0];
        let ask = [100.0, 100.0];
        let direction = [1_i8, -1];
        let nan = [f64::NAN; 2];
        let inp = inputs(&bid, &ask, &direction, &nan, &nan);

        let run_max0 = simulate_ticks(
            &inp,
            4_500.0,
            1.0,
            TickSizing::FixedSafetyMargin {
                margin_per_contract: 2_000.0,
                min_contracts: 1,
                max_contracts: 0,
            },
        )
        .unwrap();
        assert_eq!(run_max0.trades.quantity, vec![2.0]);

        let run_max1 = simulate_ticks(
            &inp,
            4_500.0,
            1.0,
            TickSizing::FixedSafetyMargin {
                margin_per_contract: 2_000.0,
                min_contracts: 1,
                max_contracts: 1,
            },
        )
        .unwrap();
        assert_eq!(run_max1.trades.quantity, vec![1.0]);
    }

    #[test]
    fn safety_margin_negative_capital_takes_min_when_qty_zero() {
        let bid = [100.0, 100.0];
        let ask = [100.0, 100.0];
        let direction = [1_i8, -1];
        let nan = [f64::NAN; 2];
        let run = simulate_ticks(
            &inputs(&bid, &ask, &direction, &nan, &nan),
            -500.0,
            1.0,
            TickSizing::FixedSafetyMargin {
                margin_per_contract: 2_000.0,
                min_contracts: 1,
                max_contracts: 0,
            },
        )
        .unwrap();
        assert_eq!(run.trades.quantity, vec![1.0]);
    }

    #[test]
    fn safety_margin_below_min_with_nonzero_qty_trades_nothing() {
        let bid = [100.0, 100.0];
        let ask = [100.0, 100.0];
        let direction = [1_i8, -1];
        let nan = [f64::NAN; 2];
        let run = simulate_ticks(
            &inputs(&bid, &ask, &direction, &nan, &nan),
            3_000.0,
            1.0,
            TickSizing::FixedSafetyMargin {
                margin_per_contract: 2_000.0,
                min_contracts: 2,
                max_contracts: 0,
            },
        )
        .unwrap();
        assert!(run.trades.quantity.is_empty());
        assert_eq!(run.final_capital.to_bits(), 3_000.0_f64.to_bits());
    }

    #[test]
    fn length_mismatch_names_ask() {
        let bid = [100.0, 100.0];
        let ask = [100.0];
        let direction = [1_i8, 0];
        let sl = [0.5, 0.5];
        let tp = [f64::NAN; 2];
        let err = simulate_ticks(
            &inputs(&bid, &ask, &direction, &sl, &tp),
            100_000.0,
            1.0,
            TickSizing::FixedQuantity { quantity: 1.0 },
        )
        .unwrap_err();
        assert_eq!(
            err,
            TickError::LengthMismatch {
                column: "ask",
                expected: 2,
                actual: 1,
            }
        );
    }
}
