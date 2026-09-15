//! The candle bar loop: `BacktestEngine._run_single_chunk` over columns.

use crate::exits::{ExitInputs, ExitRuleId, ExitRuleSet, PositionKey, Side};

use super::decision::{Decision, DecisionStep, QueuedEntry, TradeView};
use super::inputs::CandleInputs;
use super::{side_cost, CandleConfig, CandleError};

/// Microseconds in one calendar day.
const US_PER_DAY: i64 = 86_400_000_000;

/// Why a trade closed; `code` is the fixture's exit code and `as_str` the engine's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitReason {
    Rule(ExitRuleId),
    Signal,
    EndOfDay,
    ForceClose,
}

impl ExitReason {
    pub fn code(self) -> i64 {
        match self {
            Self::Rule(rule) => rule as i64,
            Self::Signal => 11,
            Self::EndOfDay => 12,
            Self::ForceClose => 13,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rule(rule) => rule.as_str(),
            Self::Signal => "SIGNAL",
            Self::EndOfDay => "END_OF_DAY",
            Self::ForceClose => "FORCE_CLOSE",
        }
    }
}

/// One row per trade in the order trades were opened.
#[derive(Clone, Debug, Default)]
pub struct TradeLedger {
    pub entry_bar: Vec<i64>,
    /// `-1` = still open.
    pub exit_bar: Vec<i64>,
    pub side: Vec<i8>,
    pub quantity: Vec<f64>,
    pub entry_price: Vec<f64>,
    /// NaN = still open.
    pub exit_price: Vec<f64>,
    pub commission: Vec<f64>,
    /// NaN = still open.
    pub pnl: Vec<f64>,
    /// [`ExitReason::code`], `-1` = still open.
    pub exit_reason: Vec<i64>,
}

/// Per bar: `exit_reason[exit_offsets[i]..exit_offsets[i + 1]]` and the entry.
///
/// Bars that were skipped or gated queue nothing and contribute an empty range.
#[derive(Clone, Debug, Default)]
pub struct DecisionTrace {
    /// Length `n + 1`.
    pub exit_offsets: Vec<i64>,
    /// Rule code, or `11` for an exit with no rule reason.
    pub exit_reason: Vec<i64>,
    pub entry: Vec<i8>,
    pub entry_strength: Vec<f64>,
}

/// Everything one run produces.
#[derive(Clone, Debug, Default)]
pub struct CandleRun {
    pub trades: TradeLedger,
    pub trace: DecisionTrace,
}

/// One trade while the loop holds it; the ledger is this vector projected into columns.
#[derive(Clone, Copy, Debug)]
struct Trade {
    entry_bar: usize,
    side: Side,
    quantity: f64,
    entry_price: f64,
    commission: f64,
    open: bool,
    exit_bar: i64,
    exit_price: f64,
    pnl: f64,
    exit_reason: i64,
}

/// `TradeRegistry.close_trade` plus `_close_trade_with_costs`: charge the exit side, then
/// price difference times quantity times point value, less the whole commission.
///
/// The commission is subtracted in its own step, as Python's `trade.pnl -= trade.commission`
/// does, so nothing here can fuse into a different result.
fn close_trade(
    trade: &mut Trade,
    bar: usize,
    price: f64,
    reason: ExitReason,
    config: &CandleConfig,
) -> f64 {
    trade.commission += side_cost(config.costs, price, trade.quantity, config.point_value);
    let mut pnl = match trade.side {
        Side::Long => ((price - trade.entry_price) * trade.quantity) * config.point_value,
        Side::Short => ((trade.entry_price - price) * trade.quantity) * config.point_value,
    };
    pnl -= trade.commission;

    trade.open = false;
    trade.exit_bar = bar as i64;
    trade.exit_price = price;
    trade.pnl = pnl;
    trade.exit_reason = reason.code();
    pnl
}

/// Close every open trade at one price, as sections A, B and E do.
fn close_all(
    trades: &mut [Trade],
    bar: usize,
    price: f64,
    reason: ExitReason,
    config: &CandleConfig,
    capital: &mut f64,
) {
    for trade in trades.iter_mut().filter(|trade| trade.open) {
        let pnl = close_trade(trade, bar, price, reason, config);
        *capital += pnl;
    }
}

/// `row.get("open", row.get("close", 0.0))`: a present column is read even where it is NaN.
fn fill_price(inputs: &CandleInputs<'_>, bar: usize) -> f64 {
    match inputs.open {
        Some(open) => open[bar],
        None => inputs.close.map_or(0.0, |close| close[bar]),
    }
}

fn calendar_day(time_us: i64) -> i64 {
    time_us.div_euclid(US_PER_DAY)
}

fn time_of_day(time_us: i64) -> i64 {
    time_us.rem_euclid(US_PER_DAY)
}

pub fn run_candle(
    inputs: &CandleInputs<'_>,
    config: &CandleConfig,
) -> Result<CandleRun, CandleError> {
    validate(inputs, config)?;

    // The exit rules read a close column. A series without one evaluates them at its fill
    // prices, which is what the backend's `row.get("close", ...)` fallbacks resolve to. The
    // fallback is owned here so that it outlives the `ExitInputs` borrowing it.
    match inputs.close {
        Some(close) => run_bars(inputs, config, close),
        None => {
            let fill_prices: Vec<f64> = (0..inputs.time_us.len())
                .map(|bar| fill_price(inputs, bar))
                .collect();
            run_bars(inputs, config, &fill_prices)
        }
    }
}

fn run_bars<'a, 'c>(
    inputs: &CandleInputs<'a>,
    config: &CandleConfig,
    close_for_exits: &'c [f64],
) -> Result<CandleRun, CandleError>
where
    'a: 'c,
{
    let n = inputs.time_us.len();
    let times = inputs.time_us;

    let rules = ExitRuleSet::new(config.exit_params.clone());
    // `CandleInputs` is invariant in its lifetime, so the caller's column lookup is re-borrowed
    // here at the shorter lifetime the close fallback may have.
    let lookup = |name: &str| -> Option<&'c [f64]> { (inputs.columns)(name) };
    let exit_inputs =
        ExitInputs::from_slices(close_for_exits, inputs.high, inputs.low, &lookup, &rules)
            .map_err(CandleError::Exit)?;
    let mut step = DecisionStep::new(config.exit_params.clone());

    let mut trades: Vec<Trade> = Vec::new();
    let mut capital = config.initial_capital;
    let mut queued = Decision::default();

    let mut trace = DecisionTrace {
        exit_offsets: Vec::with_capacity(n + 1),
        exit_reason: Vec::new(),
        entry: Vec::with_capacity(n),
        entry_strength: Vec::with_capacity(n),
    };
    trace.exit_offsets.push(0);

    for bar in 0..n {
        let mut decided = Decision::default();

        // Warm-up bars take no action at all and leave the queue untouched, as the engine's
        // `continue` before section A does.
        if inputs.tradable.is_none_or(|tradable| tradable[bar]) {
            let fill = fill_price(inputs, bar);
            let now = time_of_day(times[bar]);
            let last_bar_of_day =
                bar + 1 == n || calendar_day(times[bar + 1]) != calendar_day(times[bar]);

            let force_close_now = config
                .day_trade
                .is_some_and(|window| now >= window.force_close_us);

            if force_close_now {
                // A. Force-close at close time, ahead of everything queued.
                close_all(
                    &mut trades,
                    bar,
                    fill,
                    ExitReason::EndOfDay,
                    config,
                    &mut capital,
                );
                queued = Decision::default();
            } else {
                // B. The first queued close takes out every open trade under its own reason;
                // the Python loop's later closes find nothing open on the symbol.
                if let Some(first) = queued.exits.first() {
                    let reason = first.rule.map_or(ExitReason::Signal, ExitReason::Rule);
                    close_all(&mut trades, bar, fill, reason, config, &mut capital);
                }

                // C. Size and open the queued entry.
                if let Some(entry) = queued.entry {
                    open_entry(&mut trades, bar, fill, entry, inputs, config, capital);
                }

                // D. Evaluate this now-closed bar and queue for the next one.
                decided = evaluate_bar(
                    &mut step,
                    bar,
                    &trades,
                    inputs,
                    &exit_inputs,
                    config,
                    now,
                    last_bar_of_day,
                );
                queued = decided.clone();

                // E. Daily force-close on the last bar of the day. Section D has already
                // cleared the queue on such a bar, so this only closes trades.
                if config.day_trade.is_some() && last_bar_of_day {
                    let close = inputs.close.map_or(fill, |close| close[bar]);
                    close_all(
                        &mut trades,
                        bar,
                        close,
                        ExitReason::EndOfDay,
                        config,
                        &mut capital,
                    );
                    queued = Decision::default();
                }
            }
        }

        for exit in &decided.exits {
            trace.exit_reason.push(
                exit.rule
                    .map_or(ExitReason::Signal, ExitReason::Rule)
                    .code(),
            );
        }
        trace.exit_offsets.push(trace.exit_reason.len() as i64);
        let (entry, strength) = match decided.entry {
            Some(entry) => (
                match entry.side {
                    Side::Long => 1,
                    Side::Short => -1,
                },
                entry.strength,
            ),
            None => (0, 0.0),
        };
        trace.entry.push(entry);
        trace.entry_strength.push(strength);
    }

    // 3. End of chunk force close. The engine does not add these profits to capital, and
    // nothing sizes after them, so the omission is only visible as an absence here.
    if config.force_close_at_end && n > 0 {
        let last = n - 1;
        let price = inputs.close.map_or(0.0, |close| close[last]);
        for trade in trades.iter_mut().filter(|trade| trade.open) {
            close_trade(trade, last, price, ExitReason::ForceClose, config);
        }
    }

    Ok(CandleRun {
        trades: ledger(&trades),
        trace,
    })
}

/// Section C: size at the fill price with the current capital, trim to the cap, open.
fn open_entry(
    trades: &mut Vec<Trade>,
    bar: usize,
    fill: f64,
    entry: QueuedEntry,
    inputs: &CandleInputs<'_>,
    config: &CandleConfig,
    capital: f64,
) {
    let volatility = inputs.volatility.map(|column| column[bar]);
    let Some(mut quantity) = config
        .sizing
        .size(entry.strength, fill, capital, volatility)
    else {
        return;
    };

    if let Some(max_size) = config.sizing.max_position(fill, capital) {
        // The cap counts both sides, as the engine's sum over the symbol's open trades does.
        let open_quantity: f64 = trades
            .iter()
            .filter(|trade| trade.open)
            .map(|trade| trade.quantity)
            .sum();
        let remaining = max_size - open_quantity;
        if remaining <= 0.0 {
            return;
        }
        if quantity > remaining {
            quantity = remaining;
        }
    }

    trades.push(Trade {
        entry_bar: bar,
        side: entry.side,
        quantity,
        entry_price: fill,
        commission: side_cost(config.costs, fill, quantity, config.point_value),
        open: true,
        exit_bar: -1,
        exit_price: f64::NAN,
        pnl: f64::NAN,
        exit_reason: -1,
    });
}

/// Section D, with the engine's day-trade gating around the decision step.
#[allow(
    clippy::too_many_arguments,
    reason = "the loop's section D reads this much state"
)]
fn evaluate_bar(
    step: &mut DecisionStep,
    bar: usize,
    trades: &[Trade],
    inputs: &CandleInputs<'_>,
    exit_inputs: &ExitInputs<'_>,
    config: &CandleConfig,
    now: i64,
    last_bar_of_day: bool,
) -> Decision {
    let Some(window) = config.day_trade else {
        return decide(step, bar, trades, inputs, exit_inputs, config);
    };

    if now >= window.force_close_us || last_bar_of_day {
        return Decision::default();
    }

    let mut decision = decide(step, bar, trades, inputs, exit_inputs, config);
    if now < window.entry_start_us || now > window.entry_end_us {
        decision.entry = None;
    }
    decision
}

fn decide(
    step: &mut DecisionStep,
    bar: usize,
    trades: &[Trade],
    inputs: &CandleInputs<'_>,
    exit_inputs: &ExitInputs<'_>,
    config: &CandleConfig,
) -> Decision {
    // A trade's key is its ordinal in the ledger, which never moves: the vector is append-only.
    let open: Vec<TradeView> = trades
        .iter()
        .enumerate()
        .filter(|(_, trade)| trade.open)
        .map(|(ordinal, trade)| TradeView {
            key: PositionKey(ordinal as u64),
            side: trade.side,
            entry_price: trade.entry_price,
            entry_bar: Some(trade.entry_bar),
        })
        .collect();
    step.decide(
        bar,
        &open,
        &inputs.signals,
        exit_inputs,
        config.holding_period_bars,
    )
}

fn ledger(trades: &[Trade]) -> TradeLedger {
    let mut ledger = TradeLedger::default();
    for trade in trades {
        ledger.entry_bar.push(trade.entry_bar as i64);
        ledger.exit_bar.push(trade.exit_bar);
        ledger.side.push(match trade.side {
            Side::Long => 1,
            Side::Short => -1,
        });
        ledger.quantity.push(trade.quantity);
        ledger.entry_price.push(trade.entry_price);
        ledger.exit_price.push(trade.exit_price);
        ledger.commission.push(trade.commission);
        ledger.pnl.push(trade.pnl);
        ledger.exit_reason.push(trade.exit_reason);
    }
    ledger
}

fn validate(inputs: &CandleInputs<'_>, config: &CandleConfig) -> Result<(), CandleError> {
    config.sizing.validate()?;

    let n = inputs.time_us.len();
    let check = |column: &str, actual: usize| -> Result<(), CandleError> {
        if actual == n {
            return Ok(());
        }
        Err(CandleError::LengthMismatch {
            column: column.to_string(),
            expected: n,
            actual,
        })
    };
    for (column, series) in [
        ("open", inputs.open),
        ("high", inputs.high),
        ("low", inputs.low),
        ("close", inputs.close),
        ("volatility", inputs.volatility),
    ] {
        if let Some(series) = series {
            check(column, series.len())?;
        }
    }
    let signals = &inputs.signals;
    check("entry", signals.entry.len())?;
    check("exit_long", signals.exit_long.len())?;
    check("exit_short", signals.exit_short.len())?;
    check("strength", signals.strength.len())?;
    if let Some(bar_index) = signals.bar_index {
        check("bar_index", bar_index.len())?;
    }
    if let Some(tradable) = inputs.tradable {
        check("tradable", tradable.len())?;
    }

    for (bar, &entry) in signals.entry.iter().enumerate() {
        if !(-1..=1).contains(&entry) {
            return Err(CandleError::InvalidSignal {
                column: "entry",
                bar,
                reason: "must be -1, 0 or 1",
            });
        }
    }
    // Python's validator checks entry bars first, then the whole column.
    for bar in 0..n {
        if signals.entry[bar] == 0 {
            continue;
        }
        let strength = signals.strength[bar];
        if strength.is_nan() {
            return Err(CandleError::InvalidSignal {
                column: "strength",
                bar,
                reason: "NaN on an entry bar",
            });
        }
        if strength < 0.0 || strength > 1.0 {
            return Err(CandleError::InvalidSignal {
                column: "strength",
                bar,
                reason: "out of [0, 1] on an entry bar",
            });
        }
    }
    for (bar, &strength) in signals.strength.iter().enumerate() {
        if strength.is_nan() {
            return Err(CandleError::InvalidSignal {
                column: "strength",
                bar,
                reason: "contains NaN",
            });
        }
    }

    if config.holding_period_bars.is_some() {
        if signals.bar_index.is_none() {
            return Err(CandleError::InvalidSignal {
                column: "bar_index",
                bar: 0,
                reason: "required when holding_period_bars is set",
            });
        }
        for (column, series) in [
            ("exit_long", signals.exit_long),
            ("exit_short", signals.exit_short),
        ] {
            if let Some(bar) = series.iter().position(|&flag| flag) {
                return Err(CandleError::InvalidSignal {
                    column,
                    bar,
                    reason: "must be all-false while a holding period is declared",
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candle::{Costs, DayTradeWindow, SignalColumns, Sizing};
    use crate::exits::{ExitParams, ParamValue};

    const HOUR: i64 = 3_600_000_000;

    /// Wall-clock microseconds for `day` days after the epoch at `hour`:00.
    fn at(day: i64, hour: i64) -> i64 {
        (day * US_PER_DAY) + (hour * HOUR)
    }

    fn bits(values: &[f64]) -> Vec<u64> {
        values.iter().map(|value| value.to_bits()).collect()
    }

    /// Hand-built series; every column is owned so tests can edit single bars.
    struct Series {
        time_us: Vec<i64>,
        open: Option<Vec<f64>>,
        high: Option<Vec<f64>>,
        low: Option<Vec<f64>>,
        close: Option<Vec<f64>>,
        entry: Vec<i8>,
        exit_long: Vec<bool>,
        exit_short: Vec<bool>,
        strength: Vec<f64>,
        bar_index: Option<Vec<i64>>,
        volatility: Option<Vec<f64>>,
        tradable: Option<Vec<bool>>,
    }

    impl Series {
        /// One bar per hour from 10:00 on the epoch day, with the given open prices.
        fn hourly(open: &[f64]) -> Self {
            let n = open.len();
            Self {
                time_us: (0..n as i64).map(|i| at(0, 10 + i)).collect(),
                open: Some(open.to_vec()),
                high: None,
                low: None,
                close: None,
                entry: vec![0; n],
                exit_long: vec![false; n],
                exit_short: vec![false; n],
                strength: vec![0.0; n],
                bar_index: None,
                volatility: None,
                tradable: None,
            }
        }

        fn enter_long(mut self, bar: usize, strength: f64) -> Self {
            self.entry[bar] = 1;
            self.strength[bar] = strength;
            self
        }

        fn enter_short(mut self, bar: usize, strength: f64) -> Self {
            self.entry[bar] = -1;
            self.strength[bar] = strength;
            self
        }

        fn run(&self, config: &CandleConfig) -> Result<CandleRun, CandleError> {
            let columns = |_: &str| None;
            let inputs = CandleInputs {
                time_us: &self.time_us,
                open: self.open.as_deref(),
                high: self.high.as_deref(),
                low: self.low.as_deref(),
                close: self.close.as_deref(),
                signals: SignalColumns {
                    entry: &self.entry,
                    exit_long: &self.exit_long,
                    exit_short: &self.exit_short,
                    strength: &self.strength,
                    bar_index: self.bar_index.as_deref(),
                },
                volatility: self.volatility.as_deref(),
                tradable: self.tradable.as_deref(),
                columns: &columns,
            };
            run_candle(&inputs, config)
        }

        fn expect(&self, config: &CandleConfig) -> CandleRun {
            self.run(config).expect("hand-built series is valid")
        }
    }

    fn config(sizing: Sizing) -> CandleConfig {
        CandleConfig {
            initial_capital: 10_000.0,
            point_value: 1.0,
            costs: None,
            sizing,
            holding_period_bars: None,
            exit_params: ExitParams::default(),
            day_trade: None,
            force_close_at_end: false,
        }
    }

    fn fixed(quantity: f64) -> Sizing {
        Sizing::FixedQuantity {
            quantity,
            scale_by_strength: false,
        }
    }

    fn day_trade() -> DayTradeWindow {
        DayTradeWindow {
            entry_start_us: 9 * HOUR,
            entry_end_us: 16 * HOUR,
            force_close_us: 17 * HOUR,
        }
    }

    #[test]
    fn force_close_time_closes_at_the_open_and_queues_nothing() {
        // 10:00, 11:00 and 17:00 on one day; the third bar is at the force-close time.
        let mut series = Series::hourly(&[100.0, 100.0, 105.0]).enter_long(0, 1.0);
        series.time_us[2] = at(0, 17);
        series.exit_long[2] = true;
        let mut config = config(fixed(1.0));
        config.day_trade = Some(day_trade());

        let run = series.expect(&config);

        assert_eq!(run.trades.entry_bar, vec![1]);
        assert_eq!(run.trades.exit_bar, vec![2]);
        assert_eq!(bits(&run.trades.exit_price), bits(&[105.0]));
        assert_eq!(run.trades.exit_reason, vec![ExitReason::EndOfDay.code()]);
        assert_eq!(bits(&run.trades.pnl), bits(&[5.0]));
        assert_eq!(
            run.trace.exit_offsets,
            vec![0, 0, 0, 0],
            "the force-close bar evaluates nothing, so its exit column is never read"
        );
        assert_eq!(run.trace.entry, vec![1, 0, 0]);
    }

    #[test]
    fn a_queued_long_exit_closes_an_open_short_with_the_signal_reason() {
        // A cap of 2 with sizes of 1 lets a long and a short be open at once.
        let mut series = Series::hourly(&[100.0, 100.0, 100.0, 110.0])
            .enter_long(0, 0.5)
            .enter_short(1, 0.5);
        series.exit_long[2] = true;
        let config = config(Sizing::FixedQuantity {
            quantity: 2.0,
            scale_by_strength: true,
        });

        let run = series.expect(&config);

        assert_eq!(run.trades.side, vec![1, -1]);
        assert_eq!(run.trades.entry_bar, vec![1, 2]);
        assert_eq!(run.trades.exit_bar, vec![3, 3]);
        assert_eq!(
            run.trades.exit_reason,
            vec![ExitReason::Signal.code(), ExitReason::Signal.code()],
            "one queued close takes out both sides under the first close's reason"
        );
        assert_eq!(bits(&run.trades.pnl), bits(&[10.0, -10.0]));
        assert_eq!(run.trace.exit_offsets, vec![0, 0, 0, 1, 1]);
        assert_eq!(run.trace.exit_reason, vec![ExitReason::Signal.code()]);
    }

    #[test]
    fn an_exit_and_an_entry_on_one_bar_fill_the_exit_first() {
        let mut series = Series::hourly(&[100.0, 100.0, 110.0, 110.0]).enter_long(0, 1.0);
        // The same bar queues the close of the open long and a fresh long.
        series.exit_long[1] = true;
        series = series.enter_long(1, 1.0);
        let config = config(fixed(1.0));

        let run = series.expect(&config);

        assert_eq!(
            run.trades.entry_bar,
            vec![1, 2],
            "the second entry only fits under the cap because the exit filled first"
        );
        assert_eq!(run.trades.exit_bar, vec![2, -1]);
        assert_eq!(bits(&run.trades.entry_price), bits(&[100.0, 110.0]));
        assert_eq!(run.trades.exit_reason, vec![ExitReason::Signal.code(), -1]);
    }

    #[test]
    fn the_cap_trims_the_entry_to_what_remains_and_skips_it_at_zero() {
        // Cap 1.5, sized 1.0: the second entry trims to 0.5 and the third finds nothing left.
        let mut series = Series::hourly(&[100.0, 100.0, 100.0, 100.0, 100.0]);
        for bar in 0..4 {
            series = series.enter_long(bar, 1.0);
        }
        let config = config(fixed(1.5));

        let run = series.expect(&config);

        assert_eq!(bits(&run.trades.quantity), bits(&[1.0, 0.5]));
        assert_eq!(run.trades.entry_bar, vec![1, 2]);
    }

    #[test]
    fn a_winning_trade_raises_the_next_safety_margin_size() {
        let mut series = Series::hourly(&[100.0, 100.0, 2_600.0, 2_600.0]).enter_long(0, 1.0);
        series.exit_long[1] = true;
        series = series.enter_long(1, 1.0);
        let config = config(Sizing::FixedSafetyMargin {
            margin_per_contract: 5_000.0,
            min_contracts: 0,
            max_contracts: None,
            scale_by_strength: false,
        });

        let run = series.expect(&config);

        assert_eq!(
            bits(&run.trades.quantity),
            bits(&[2.0, 3.0]),
            "capital 10,000 sizes 2 contracts; 5,000 of profit sizes 3"
        );
        assert_eq!(bits(&run.trades.pnl[..1]), bits(&[5_000.0]));
    }

    #[test]
    fn a_non_tradable_bar_queues_nothing_even_with_an_entry_signal() {
        let mut series = Series::hourly(&[100.0, 100.0, 100.0]).enter_long(0, 1.0);
        series.tradable = Some(vec![false, true, true]);
        let config = config(fixed(1.0));

        let run = series.expect(&config);

        assert!(run.trades.entry_bar.is_empty());
        assert_eq!(run.trace.entry, vec![0, 0, 0]);
        assert_eq!(run.trace.exit_offsets, vec![0, 0, 0, 0]);
    }

    #[test]
    fn force_close_at_end_closes_at_the_last_close_and_otherwise_leaves_it_open() {
        let mut series = Series::hourly(&[100.0, 100.0, 100.0]).enter_long(0, 1.0);
        series.close = Some(vec![100.0, 100.0, 120.0]);
        let mut config = config(fixed(1.0));

        let open = series.expect(&config);
        assert_eq!(open.trades.exit_bar, vec![-1]);
        assert_eq!(open.trades.exit_reason, vec![-1]);
        assert!(open.trades.exit_price[0].is_nan());
        assert!(open.trades.pnl[0].is_nan());

        config.force_close_at_end = true;
        let closed = series.expect(&config);
        assert_eq!(closed.trades.exit_bar, vec![2]);
        assert_eq!(bits(&closed.trades.exit_price), bits(&[120.0]));
        assert_eq!(
            closed.trades.exit_reason,
            vec![ExitReason::ForceClose.code()]
        );
        assert_eq!(bits(&closed.trades.pnl), bits(&[20.0]));
    }

    #[test]
    fn the_last_bar_of_a_day_closes_at_its_close_not_its_open() {
        let mut series = Series::hourly(&[100.0, 100.0, 105.0]).enter_long(0, 1.0);
        series.close = Some(vec![100.0, 100.0, 130.0]);
        let mut config = config(fixed(1.0));
        config.day_trade = Some(day_trade());

        let run = series.expect(&config);

        assert_eq!(run.trades.exit_bar, vec![2]);
        assert_eq!(bits(&run.trades.exit_price), bits(&[130.0]));
        assert_eq!(run.trades.exit_reason, vec![ExitReason::EndOfDay.code()]);
        assert_eq!(bits(&run.trades.pnl), bits(&[30.0]));
    }

    #[test]
    fn a_bar_before_the_epoch_ends_its_own_calendar_day() {
        // 10:00 the day before the epoch, then 10:00 and 11:00 on the epoch day.
        let mut series = Series::hourly(&[100.0, 100.0, 100.0])
            .enter_long(0, 1.0)
            .enter_long(1, 1.0);
        series.time_us[0] = at(-1, 10);
        let mut config = config(fixed(1.0));
        config.day_trade = Some(day_trade());

        let run = series.expect(&config);

        assert_eq!(
            run.trace.entry,
            vec![0, 1, 0],
            "the pre-epoch bar is the last of its day, so it queues nothing"
        );
        assert_eq!(
            run.trades.entry_bar,
            vec![2],
            "only the entry queued on the epoch day fills"
        );
    }

    #[test]
    fn costs_are_charged_on_both_sides_of_a_trade() {
        let mut series = Series::hourly(&[100.0, 100.0, 110.0]).enter_long(0, 1.0);
        series.exit_long[1] = true;
        let mut config = config(fixed(2.0));
        config.costs = Some(Costs {
            per_contract: 1.5,
            bps: 3.0,
        });
        config.point_value = 0.2;

        let run = series.expect(&config);

        let entry_cost = side_cost(config.costs, 100.0, 2.0, 0.2);
        let exit_cost = side_cost(config.costs, 110.0, 2.0, 0.2);
        let mut commission = entry_cost;
        commission += exit_cost;
        assert_eq!(bits(&run.trades.commission), bits(&[commission]));
        let mut pnl = ((110.0_f64 - 100.0) * 2.0) * 0.2;
        pnl -= commission;
        assert_eq!(bits(&run.trades.pnl), bits(&[pnl]));
    }

    #[test]
    fn a_holding_period_closes_the_trade_and_needs_a_bar_index() {
        let series = Series::hourly(&[100.0, 100.0, 100.0, 100.0, 110.0]).enter_long(0, 1.0);
        let mut config = config(fixed(1.0));
        config.holding_period_bars = Some(2);

        let missing = series.run(&config).expect_err("bar_index is required");
        assert_eq!(
            missing,
            CandleError::InvalidSignal {
                column: "bar_index",
                bar: 0,
                reason: "required when holding_period_bars is set",
            }
        );

        let mut series = series;
        series.bar_index = Some(vec![0, 1, 2, 3, 4]);
        let run = series.expect(&config);
        assert_eq!(run.trades.entry_bar, vec![1]);
        assert_eq!(
            run.trades.exit_bar,
            vec![4],
            "bar 3 reaches the period and the close fills on bar 4"
        );
        assert_eq!(run.trades.exit_reason, vec![ExitReason::Signal.code()]);
    }

    #[test]
    fn a_rule_exit_records_its_rule_as_the_reason() {
        let mut series = Series::hourly(&[100.0, 100.0, 100.0, 100.0]).enter_long(0, 1.0);
        series.close = Some(vec![100.0, 100.0, 85.0, 100.0]);
        let mut config = config(fixed(1.0));
        config.exit_params =
            ExitParams::from_pairs([("stop_loss_pct", ParamValue::Float(0.10))]).unwrap();

        let run = series.expect(&config);

        assert_eq!(run.trades.exit_bar, vec![3]);
        assert_eq!(
            run.trades.exit_reason,
            vec![ExitReason::Rule(ExitRuleId::FixedStopLoss).code()]
        );
        assert_eq!(
            run.trace.exit_reason,
            vec![ExitReason::Rule(ExitRuleId::FixedStopLoss).code()]
        );
    }

    #[test]
    fn a_close_only_series_fills_at_the_close() {
        let mut series = Series::hourly(&[100.0, 100.0, 120.0]).enter_long(0, 1.0);
        series.close = series.open.take();
        let config = config(fixed(1.0));

        let run = series.expect(&config);

        assert_eq!(bits(&run.trades.entry_price), bits(&[100.0]));
        assert_eq!(run.trades.entry_bar, vec![1]);
    }

    #[test]
    fn an_entry_outside_the_contract_is_rejected() {
        let mut series = Series::hourly(&[100.0, 100.0]);
        series.entry[1] = 2;
        let config = config(fixed(1.0));

        assert_eq!(
            series
                .run(&config)
                .expect_err("entry 2 is not in the contract"),
            CandleError::InvalidSignal {
                column: "entry",
                bar: 1,
                reason: "must be -1, 0 or 1",
            }
        );
    }

    #[test]
    fn a_short_strength_column_is_named_in_the_error() {
        let mut series = Series::hourly(&[100.0, 100.0, 100.0]);
        series.strength.pop();
        let config = config(fixed(1.0));

        assert_eq!(
            series.run(&config).expect_err("strength is one short"),
            CandleError::LengthMismatch {
                column: "strength".to_string(),
                expected: 3,
                actual: 2,
            }
        );
    }
}
