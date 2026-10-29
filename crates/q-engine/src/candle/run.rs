//! The candle bar loop: `BacktestEngine._run_single_chunk` over columns.

use crate::exits::breakeven::BreakevenStop;
use crate::exits::{ExitInputs, ExitRuleId, ExitRuleSet, PositionKey, Side};

use super::decision::{queued_entry, Decision, DecisionStep, QueuedEntry, QueuedExit, TradeView};
use super::inputs::{BarSignals, CandleInputs, EntryCandidate};
use super::protective::{
    priced_fill, IntrabarPrices, IntrabarSource, Levels, RejectedEntries, RestingKind, RestingOrder,
};
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
    /// An intrabar stop level was reached.
    StopLoss,
    /// An intrabar target level was traded through.
    TakeProfit,
    /// The pre-activation breakeven emergency stop was reached intrabar.
    BreakevenEmergency,
}

impl ExitReason {
    pub fn code(self) -> i64 {
        match self {
            Self::Rule(rule) => rule as i64,
            Self::Signal => 11,
            Self::EndOfDay => 12,
            Self::ForceClose => 13,
            Self::StopLoss => 14,
            Self::TakeProfit => 15,
            Self::BreakevenEmergency => 16,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rule(rule) => rule.as_str(),
            Self::Signal => "SIGNAL",
            Self::EndOfDay => "END_OF_DAY",
            Self::ForceClose => "FORCE_CLOSE",
            Self::StopLoss => "STOP_LOSS",
            Self::TakeProfit => "TAKE_PROFIT",
            Self::BreakevenEmergency => "breakeven_emergency",
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
    /// Time of the price that triggered an intrabar exit; `-1` for closed-bar exits.
    pub exit_time_us: Vec<i64>,
    /// How the trade opened: `0` open fill, `1` priced, `2` stop order, `3` limit order.
    pub entry_kind: Vec<i64>,
    /// Time of the price that filled a resting order; `-1` for every other entry.
    pub entry_time_us: Vec<i64>,
    /// Stop and target in force when the trade closed, or now if it is still open. `NaN` = unset.
    pub final_stop_price: Vec<f64>,
    pub final_target_price: Vec<f64>,
    /// Index of the entry candidate that filled on its deciding bar; `-1` for every other entry.
    pub entry_candidate: Vec<i64>,
    /// Index of the exit price that closed the trade; `-1` for every other exit.
    pub exit_candidate: Vec<i64>,
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
    /// Entries refused by a wrong-side protective level or by a missing post-exit touch.
    pub rejected: RejectedEntries,
}

/// How a trade was opened. The code is the ledger's `entry_kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryKind {
    Open,
    Priced,
    Stop,
    Limit,
}

impl EntryKind {
    fn code(self) -> i64 {
        match self {
            Self::Open => 0,
            Self::Priced => 1,
            Self::Stop => 2,
            Self::Limit => 3,
        }
    }
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
    levels: Levels,
    exit_time_us: i64,
    breakeven: BreakevenStop,
    kind: EntryKind,
    entry_time_us: i64,
    /// The scan tick that filled the trade on its entry bar. That price is never its exit.
    first_tick: usize,
    entry_candidate: i64,
    exit_candidate: i64,
}

impl Trade {
    /// Whether a scan tick of `bar` is the trade's own fill, which cannot trigger it.
    fn skips_tick(&self, bar: usize, tick: usize) -> bool {
        self.entry_bar == bar && tick == self.first_tick
    }
}

/// A member of a working group of resting entry orders waiting for a fill.
#[derive(Clone, Copy, Debug)]
struct WorkingMember {
    side: Side,
    strength: f64,
    order: RestingOrder,
    levels: Levels,
    candidate_index: i64,
    placed_bar: usize,
    earliest_fill_us: i64,
}

impl WorkingMember {
    /// Lapsed once a bar is past `placed_bar + expire_bars`, the last bar it may fill on.
    fn lapsed_at(&self, bar: usize) -> bool {
        self.order
            .expire_bars
            .is_some_and(|bars| bar - self.placed_bar > bars)
    }

    fn expired_at(&self, time_us: i64) -> bool {
        self.order.expire_time_us.is_some_and(|exp| time_us >= exp)
    }

    fn reached_by_range(&self, high: f64, low: f64) -> bool {
        self.order.reached_by_range(self.side, high, low)
    }

    fn fill_at(&self, price: f64) -> Option<f64> {
        self.order.fill_at(self.side, price)
    }
}

/// An active timed exit closing open positions at or after `target_time_us`.
#[derive(Clone, Copy, Debug)]
struct WorkingTimedExit {
    target_time_us: i64,
    earliest_fill_us: i64,
}

/// A working group of resting entry orders (either a single resting order or an OCO group).
#[derive(Clone, Debug)]
struct WorkingGroup {
    members: Vec<WorkingMember>,
}

impl WorkingGroup {
    fn reached_by_range(&self, high: f64, low: f64) -> bool {
        self.members.iter().any(|m| m.reached_by_range(high, low))
    }
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

/// Close every open trade at one price, as sections A, B and E do. `keep` names a trade the close
/// leaves open: a priced entry opened by the decision that queued the close.
fn close_all(
    trades: &mut [Trade],
    bar: usize,
    price: f64,
    reason: ExitReason,
    config: &CandleConfig,
    capital: &mut f64,
    keep: Option<usize>,
) {
    for (index, trade) in trades.iter_mut().enumerate() {
        if !trade.open || Some(index) == keep {
            continue;
        }
        let pnl = close_trade(trade, bar, price, reason, config);
        *capital += pnl;
    }
}

/// Applies a decision's explicitly priced signal close after this bar's protective scan. Each
/// trade closes at the requested price its tape fills first, the earlier listed one on a tie, as
/// [`priced_fill`] fills a stop or a limit. The close consumes the decision even when the tape fills
/// none of them.
#[allow(
    clippy::too_many_arguments,
    reason = "the close reads the decision, its callback signal, tape, costs and running capital"
)]
fn priced_signal_exit(
    trades: &mut [Trade],
    decision: &Decision,
    signals: &BarSignals,
    bar: usize,
    requested: &[f64],
    inputs: &CandleInputs<'_>,
    config: &CandleConfig,
    capital: &mut f64,
) -> Result<(bool, Option<usize>), CandleError> {
    let selected = decision.exits.iter().find(|exit| exit.rule.is_none());
    let Some(selected) = selected else {
        return Ok((false, None));
    };
    let index = selected.key.0 as usize;
    let Some(selected_trade) = trades.get(index) else {
        return Ok((false, None));
    };
    let side = selected_trade.side;
    let requested_by_signal = match side {
        Side::Long => signals.exit_long,
        Side::Short => signals.exit_short,
    };
    if !requested_by_signal {
        return Ok((false, None));
    }

    let source = inputs.intrabar.ok_or(CandleError::InvalidConfig {
        field: "exit_price",
        reason: "requires intrabar, open, high and low".to_string(),
    })?;
    let prices = intrabar_prices(source, bar)?;
    let mut last_exit_tick = None;
    for trade in trades
        .iter_mut()
        .filter(|trade| trade.open && trade.side == side)
    {
        let first_tick = match trade.kind {
            EntryKind::Stop | EntryKind::Limit | EntryKind::Priced if trade.entry_bar == bar => {
                trade.first_tick.saturating_add(1)
            }
            _ => 0,
        }
        .min(prices.price.len());
        // Closing a long sells and closing a short buys. Tick indices preserve source order when
        // timestamps tie.
        let buy = trade.side == Side::Short;
        let reached = requested
            .iter()
            .enumerate()
            .filter_map(|(candidate, &price)| {
                priced_fill(&prices.price, price, buy, first_tick)
                    .map(|(tick, fill)| (tick, candidate, fill))
            })
            .min_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        if let Some((tick, candidate, fill)) = reached {
            let time_us = prices.time_us[tick];
            let pnl = close_trade(trade, bar, fill, ExitReason::Signal, config);
            trade.exit_time_us = time_us;
            trade.exit_candidate = candidate as i64;
            *capital += pnl;
            last_exit_tick = Some(last_exit_tick.map_or(tick, |latest: usize| latest.max(tick)));
        }
    }
    Ok((true, last_exit_tick))
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

/// Immutable view of a filled trade at the strategy decision boundary. The levels are the trade's
/// current stop and target, `NaN` where unset.
#[derive(Clone, Copy, Debug)]
pub struct PositionSnapshot {
    pub key: PositionKey,
    pub side: Side,
    pub entry_bar: usize,
    pub entry_price: f64,
    pub quantity: f64,
    pub stop_price: f64,
    pub target_price: f64,
}

pub type StrategyFn<'a, E> = dyn FnMut(usize, &[PositionSnapshot]) -> Result<BarSignals, E> + 'a;

/// Returns whether to close every open trade at the next bar's open.
pub type EntryFilledFn<'a, E> = dyn FnMut(usize, &[PositionSnapshot]) -> Result<bool, E> + 'a;

/// Host decisions consulted during a run. Absent callbacks leave the static columns in charge.
pub struct RuntimeCallbacks<'a, E> {
    /// Closed-bar decisions, called once per bar after intrabar fills. With `entry` set it decides
    /// exits only.
    pub strategy: Option<&'a mut StrategyFn<'a, E>>,
    /// Closed-bar entry decisions, called once per bar after the strategy's priced closes resolve,
    /// with the positions still open then. Only its entry fields are read.
    pub entry: Option<&'a mut StrategyFn<'a, E>>,
    /// Called after a priced entry fills on its decision bar and is still open, with that trade
    /// among the positions. The closed-bar decision ran before the fill and never saw it, so this
    /// is the only call that can close it at the next bar's open, ahead of that bar's scan.
    pub entry_filled: Option<&'a mut EntryFilledFn<'a, E>>,
}

impl<E> Default for RuntimeCallbacks<'_, E> {
    fn default() -> Self {
        Self {
            strategy: None,
            entry: None,
            entry_filled: None,
        }
    }
}

pub fn run_candle(
    inputs: &CandleInputs<'_>,
    config: &CandleConfig,
) -> Result<CandleRun, CandleError> {
    run_with_callbacks(inputs, config, RuntimeCallbacks::default())
}

/// Run the authoritative simulation with fallible decisions from the host.
/// Callback errors abort immediately; fills and rule decisions remain owned by Rust.
pub fn run_candle_with_callback<E, F>(
    inputs: &CandleInputs<'_>,
    config: &CandleConfig,
    mut callback: F,
) -> Result<CandleRun, E>
where
    E: From<CandleError>,
    F: FnMut(usize, &[PositionSnapshot]) -> Result<BarSignals, E>,
{
    run_with_callbacks(
        inputs,
        config,
        RuntimeCallbacks {
            strategy: Some(&mut callback as &mut StrategyFn<'_, E>),
            entry: None,
            entry_filled: None,
        },
    )
}

pub fn run_candle_with_callbacks<E: From<CandleError>>(
    inputs: &CandleInputs<'_>,
    config: &CandleConfig,
    callbacks: RuntimeCallbacks<'_, E>,
) -> Result<CandleRun, E> {
    run_with_callbacks(inputs, config, callbacks)
}

fn intrabar_ready(inputs: &CandleInputs<'_>) -> bool {
    inputs.intrabar.is_some()
        && inputs.open.is_some()
        && inputs.high.is_some()
        && inputs.low.is_some()
}

fn run_with_callbacks<E: From<CandleError>>(
    inputs: &CandleInputs<'_>,
    config: &CandleConfig,
    callbacks: RuntimeCallbacks<'_, E>,
) -> Result<CandleRun, E> {
    validate(inputs, config)?;
    match inputs.close {
        Some(close) => run_bars(inputs, config, close, callbacks),
        None => {
            let fill_prices: Vec<f64> = (0..inputs.time_us.len())
                .map(|bar| fill_price(inputs, bar))
                .collect();
            run_bars(inputs, config, &fill_prices, callbacks)
        }
    }
}

fn snapshot(trades: &[Trade]) -> Vec<PositionSnapshot> {
    trades
        .iter()
        .enumerate()
        .filter(|(_, trade)| trade.open)
        .map(|(ordinal, trade)| PositionSnapshot {
            key: PositionKey(ordinal as u64),
            side: trade.side,
            entry_bar: trade.entry_bar,
            entry_price: trade.entry_price,
            quantity: trade.quantity,
            stop_price: trade.levels.stop,
            target_price: trade.levels.target,
        })
        .collect()
}

fn strategy_signals<E: From<CandleError>>(
    callbacks: &mut RuntimeCallbacks<'_, E>,
    bar: usize,
    trades: &[Trade],
    inputs: &CandleInputs<'_>,
) -> Result<Option<BarSignals>, E> {
    let split = callbacks.entry.is_some();
    let Some(callback) = callbacks.strategy.as_mut() else {
        return Ok(None);
    };
    let signals = callback(bar, &snapshot(trades))?;
    if split && has_entry_fields(&signals) {
        return Err(CandleError::InvalidSignal {
            column: "entry",
            bar,
            reason: "the strategy callback decides exits only when an entry callback is set",
        }
        .into());
    }
    checked_signals(signals, bar, inputs).map(Some)
}

/// The entry callback's decision for this bar, seeing the positions left after the strategy's
/// priced closes. Its exit fields must be unset.
fn entry_signals<E: From<CandleError>>(
    callbacks: &mut RuntimeCallbacks<'_, E>,
    bar: usize,
    trades: &[Trade],
    inputs: &CandleInputs<'_>,
) -> Result<Option<BarSignals>, E> {
    let Some(callback) = callbacks.entry.as_mut() else {
        return Ok(None);
    };
    let signals = callback(bar, &snapshot(trades))?;
    if signals.exit_long
        || signals.exit_short
        || !signals.exit_prices.is_empty()
        || signals.close_next_open
        || signals.amend_stop.is_some()
        || signals.amend_target.is_some()
        || signals.timed_exit.is_some()
        || signals.cancel_timed_exit
    {
        return Err(CandleError::InvalidSignal {
            column: "exit_long",
            bar,
            reason: "the entry callback decides entries only",
        }
        .into());
    }
    checked_signals(signals, bar, inputs).map(Some)
}

fn has_entry_fields(signals: &BarSignals) -> bool {
    signals.entry != 0
        || signals.levels.is_some()
        || signals
            .entry_candidates
            .as_ref()
            .is_some_and(|candidates| !candidates.is_empty())
        || signals.resting.is_some()
        || signals.oco_group.is_some()
        || signals.cancel_entry
}

fn checked_signals<E: From<CandleError>>(
    signals: BarSignals,
    bar: usize,
    inputs: &CandleInputs<'_>,
) -> Result<BarSignals, E> {
    if (signals.amend_stop.is_some() || signals.amend_target.is_some()) && !intrabar_ready(inputs) {
        return Err(CandleError::InvalidConfig {
            field: "amend_stop/amend_target",
            reason: "requires intrabar, open, high and low".to_string(),
        }
        .into());
    }
    if let Some((stop, target)) = signals.levels {
        check_level("stop_price", bar, stop)?;
        check_level("target_price", bar, target)?;
    }
    for candidate in signals.entry_candidates.iter().flatten() {
        check_positive("entry_price", bar, candidate.price)?;
        if let Some((stop, target)) = candidate.levels {
            check_level("stop_price", bar, stop)?;
            check_level("target_price", bar, target)?;
        }
    }
    if signals
        .entry_candidates
        .as_ref()
        .is_some_and(|candidates| !candidates.is_empty())
        && (signals.entry == 0 || signals.resting.is_some())
    {
        return Err(CandleError::InvalidSignal {
            column: "entry_price",
            bar,
            reason: "priced entries need a market entry decision",
        }
        .into());
    }
    if let Some(order) = signals.resting {
        check_resting(&signals, order, bar)?;
    }
    if let Some(oco) = &signals.oco_group {
        if !intrabar_ready(inputs) {
            return Err(CandleError::InvalidConfig {
                field: "oco_group",
                reason: "requires intrabar, open, high and low".to_string(),
            }
            .into());
        }
        if oco.len() < 2 {
            return Err(CandleError::InvalidSignal {
                column: "oco_group",
                bar,
                reason: "an OCO group requires at least two orders",
            }
            .into());
        }
        if signals.resting.is_some() {
            return Err(CandleError::InvalidSignal {
                column: "oco_group",
                bar,
                reason: "cannot specify both resting and oco_group",
            }
            .into());
        }
        for order in oco {
            check_positive("entry_price", bar, order.price)?;
            if let Some((stop, target)) = order.levels {
                check_level("stop_price", bar, stop)?;
                check_level("target_price", bar, target)?;
            }
            if order.expire_bars == Some(0) {
                return Err(CandleError::InvalidSignal {
                    column: "expire_bars",
                    bar,
                    reason: "must be a positive integer",
                }
                .into());
            }
            if let Some(exp) = order.expire_time_us {
                check_positive("expire_time_us", bar, exp as f64)?;
            }
        }
    }
    if let Some(timed_exit) = signals.timed_exit {
        check_positive("timed_exit", bar, timed_exit as f64)?;
    }
    if let Some(stop) = signals.amend_stop {
        check_positive("amend_stop", bar, stop)?;
    }
    if let Some(target) = signals.amend_target {
        check_positive("amend_target", bar, target)?;
    }
    for &price in &signals.exit_prices {
        check_positive("exit_price", bar, price)?;
        let (Some(_), Some(high), Some(low)) = (inputs.open, inputs.high, inputs.low) else {
            return Err(CandleError::InvalidSignal {
                column: "exit_price",
                bar,
                reason: "requires intrabar, open, high and low",
            }
            .into());
        };
        if inputs.intrabar.is_none() {
            return Err(CandleError::InvalidSignal {
                column: "exit_price",
                bar,
                reason: "requires intrabar, open, high and low",
            }
            .into());
        }
        if !(low[bar] <= price && price <= high[bar]) {
            return Err(CandleError::InvalidSignal {
                column: "exit_price",
                bar,
                reason: "must lie within the bar's low and high",
            }
            .into());
        }
    }
    if !(-1..=1).contains(&signals.entry) {
        return Err(CandleError::InvalidSignal {
            column: "entry",
            bar,
            reason: "must be -1, 0 or 1",
        }
        .into());
    }
    if !signals.strength.is_finite() || !(0.0..=1.0).contains(&signals.strength) {
        return Err(CandleError::InvalidSignal {
            column: "strength",
            bar,
            reason: "must be finite and in [0, 1]",
        }
        .into());
    }
    Ok(signals)
}

fn run_bars<'a, 'c, E: From<CandleError>>(
    inputs: &CandleInputs<'a>,
    config: &CandleConfig,
    close_for_exits: &'c [f64],
    mut callbacks: RuntimeCallbacks<'_, E>,
) -> Result<CandleRun, E>
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
    let mut closed_bar_params = config.exit_params.clone();
    if closed_bar_params.intrabar_breakeven() {
        closed_bar_params.breakeven_trigger_pct = 0.0;
    }
    let mut step = DecisionStep::new(closed_bar_params);

    let mut trades: Vec<Trade> = Vec::new();
    let mut capital = config.initial_capital;
    let mut queued = Decision::default();
    let mut queued_levels = Levels::NONE;
    // The priced trade that the queued decision opened on its own bar. The queued close must not
    // take it: the close only clears trades the decision saw open.
    let mut queued_exempt: Option<usize> = None;
    let mut rejected = RejectedEntries::default();
    let mut working: Option<WorkingGroup> = None;
    let mut working_timed_exit: Option<WorkingTimedExit> = None;

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

            if let Some(group) = working.as_mut() {
                group.members.retain(|order| !order.lapsed_at(bar));
                if group.members.is_empty() {
                    working = None;
                }
            }

            if force_close_now {
                // A. Force-close at close time, ahead of everything queued. A working order
                // lapses with the session.
                close_all(
                    &mut trades,
                    bar,
                    fill,
                    ExitReason::EndOfDay,
                    config,
                    &mut capital,
                    None,
                );
                queued = Decision::default();
                queued_exempt = None;
                working = None;
                working_timed_exit = None;
                strategy_signals(&mut callbacks, bar, &trades, inputs)?;
                entry_signals(&mut callbacks, bar, &trades, inputs)?;
            } else {
                let mut timed_exit_first_trade = None;
                let timed_exit_at_open = working_timed_exit.is_some_and(|exit| {
                    let first_trade_time = match inputs.intrabar {
                        Some(source) => intrabar_prices::<E>(source, bar)
                            .ok()
                            .and_then(|p| p.time_us.first().copied())
                            .unwrap_or(times[bar]),
                        None => times[bar],
                    };
                    if first_trade_time >= exit.target_time_us
                        && first_trade_time >= exit.earliest_fill_us
                    {
                        timed_exit_first_trade = Some(first_trade_time);
                        true
                    } else {
                        false
                    }
                });
                if timed_exit_at_open {
                    let first_trade_time = timed_exit_first_trade.unwrap_or(times[bar]);
                    working_timed_exit = None;
                    for (index, trade) in trades.iter_mut().enumerate() {
                        if !trade.open || Some(index) == queued_exempt {
                            continue;
                        }
                        let pnl = close_trade(trade, bar, fill, ExitReason::Signal, config);
                        trade.exit_time_us = first_trade_time;
                        capital += pnl;
                    }
                } else if let Some(first) = queued.exits.first() {
                    // B. The first queued close takes out every open trade under its own reason;
                    // the Python loop's later closes find nothing open on the symbol.
                    let reason = first.rule.map_or(ExitReason::Signal, ExitReason::Rule);
                    close_all(
                        &mut trades,
                        bar,
                        fill,
                        reason,
                        config,
                        &mut capital,
                        queued_exempt,
                    );
                }

                // C. Size and open the queued entry, unless the cap or its levels refuse it.
                if let Some(entry) = queued.entry {
                    open_entry(
                        &mut trades,
                        &mut rejected,
                        bar,
                        fill,
                        entry,
                        Opening {
                            kind: EntryKind::Open,
                            levels: queued_levels,
                            time_us: -1,
                            tick: 0,
                        },
                        inputs,
                        config,
                        capital,
                    );
                }

                // Intrabar stops, targets and the resting fill share one scan of this bar's trade
                // prices, ahead of section D.
                let mut last_exit_tick = intrabar_fills(
                    &mut trades,
                    &mut working,
                    &mut working_timed_exit,
                    &mut rejected,
                    bar,
                    inputs,
                    config,
                    &mut capital,
                )?;

                let mut runtime_signals = strategy_signals(&mut callbacks, bar, &trades, inputs)?;
                // Amendments reach the trades the callback saw, from the next bar's first price.
                if let Some(signals) = &runtime_signals {
                    amend_levels(&mut trades, signals);
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
                    runtime_signals.clone(),
                );
                let mut immediate_exit = false;
                if let Some(signals) = runtime_signals
                    .as_ref()
                    .filter(|signals| !signals.exit_prices.is_empty())
                {
                    let (consumed, exit_tick) = priced_signal_exit(
                        &mut trades,
                        &decided,
                        signals,
                        bar,
                        &signals.exit_prices,
                        inputs,
                        config,
                        &mut capital,
                    )?;
                    immediate_exit = consumed && !signals.close_next_open;
                    last_exit_tick = match (last_exit_tick, exit_tick) {
                        (Some(a), Some(b)) => Some(a.max(b)),
                        (a, b) => a.or(b),
                    };
                }
                // A split host decides its entry once this bar's priced closes have resolved.
                if let Some(entry_signals) = entry_signals(&mut callbacks, bar, &trades, inputs)? {
                    decided.entry = queued_entry(&entry_signals)
                        .filter(|_| entry_allowed(config, now, last_bar_of_day));
                    runtime_signals = Some(match runtime_signals {
                        Some(exits) => BarSignals {
                            entry: entry_signals.entry,
                            strength: entry_signals.strength,
                            levels: entry_signals.levels,
                            entry_candidates: entry_signals.entry_candidates,
                            resting: entry_signals.resting,
                            cancel_entry: entry_signals.cancel_entry,
                            oco_group: entry_signals.oco_group.or(exits.oco_group),
                            timed_exit: exits.timed_exit.or(entry_signals.timed_exit),
                            cancel_timed_exit: exits.cancel_timed_exit
                                || entry_signals.cancel_timed_exit,
                            ..exits
                        },
                        None => entry_signals,
                    });
                }
                // A cancel removes the working order. Any entry decided here replaces it.
                if runtime_signals
                    .as_ref()
                    .is_some_and(|signals| signals.cancel_entry)
                {
                    working = None;
                }
                queued = decided.clone();
                if immediate_exit {
                    queued.exits.clear();
                }
                queued_exempt = None;
                // The levels and price are read from this bar only when it decides an entry.
                let runtime_levels = runtime_signals.as_ref().and_then(|signals| signals.levels);
                queued_levels = if decided.entry.is_some() {
                    entry_levels(inputs, bar, runtime_levels)?
                } else {
                    Levels::NONE
                };
                if decided.entry.is_some() {
                    working = None;
                }
                // A resting entry works from the next bar instead of queuing for its open.
                let resting = decided
                    .entry
                    .and(runtime_signals.as_ref().and_then(|signals| signals.resting));
                if let (Some(entry), Some(order)) = (decided.entry, resting) {
                    if !intrabar_ready(inputs) {
                        return Err(CandleError::InvalidConfig {
                            field: "resting_order",
                            reason: "requires intrabar, open, high and low".to_string(),
                        }
                        .into());
                    }
                    queued.entry = None;
                    let earliest_fill_us =
                        times.get(bar + 1).copied().unwrap_or(times[bar]) + config.latency_us;
                    working = Some(WorkingGroup {
                        members: vec![WorkingMember {
                            side: entry.side,
                            strength: entry.strength,
                            order,
                            levels: queued_levels,
                            candidate_index: -1,
                            placed_bar: bar,
                            earliest_fill_us,
                        }],
                    });
                }
                let oco = runtime_signals
                    .as_ref()
                    .and_then(|signals| signals.oco_group.as_ref());
                if let Some(orders) = oco {
                    if !intrabar_ready(inputs) {
                        return Err(CandleError::InvalidConfig {
                            field: "oco_group",
                            reason: "requires intrabar, open, high and low".to_string(),
                        }
                        .into());
                    }
                    let earliest_fill_us =
                        times.get(bar + 1).copied().unwrap_or(times[bar]) + config.latency_us;
                    let default_strength = runtime_signals.as_ref().map_or(1.0, |s| s.strength);
                    let mut members = Vec::with_capacity(orders.len());
                    for (index, order) in orders.iter().enumerate() {
                        let levels = match order.levels.or(runtime_levels) {
                            Some(own) => entry_levels(inputs, bar, Some(own))?,
                            None => queued_levels,
                        };
                        members.push(WorkingMember {
                            side: order.side,
                            strength: default_strength,
                            order: RestingOrder {
                                kind: order.kind,
                                price: order.price,
                                expire_bars: order.expire_bars,
                                expire_time_us: order.expire_time_us,
                            },
                            levels,
                            candidate_index: index as i64,
                            placed_bar: bar,
                            earliest_fill_us,
                        });
                    }
                    working = Some(WorkingGroup { members });
                }
                if runtime_signals
                    .as_ref()
                    .is_some_and(|signals| signals.cancel_timed_exit)
                {
                    working_timed_exit = None;
                }
                if let Some(target_time_us) = runtime_signals
                    .as_ref()
                    .and_then(|signals| signals.timed_exit)
                {
                    let earliest_fill_us =
                        times.get(bar + 1).copied().unwrap_or(times[bar]) + config.latency_us;
                    working_timed_exit = Some(WorkingTimedExit {
                        target_time_us,
                        earliest_fill_us,
                    });
                }
                // A priced entry fills on this bar, so it is never queued for the next one.
                if let Some(entry) = decided.entry.filter(|_| resting.is_none()) {
                    let runtime_candidates = runtime_signals
                        .as_ref()
                        .and_then(|signals| signals.entry_candidates.as_deref());
                    let candidates = priced_entry(
                        inputs,
                        bar,
                        entry.side,
                        runtime_candidates,
                        runtime_levels,
                        queued_levels,
                    )?;
                    if !candidates.is_empty() {
                        queued.entry = None;
                        queued_exempt = open_priced_entry(
                            &mut trades,
                            bar,
                            entry,
                            &candidates,
                            last_exit_tick,
                            inputs,
                            config,
                            &mut capital,
                            &mut rejected,
                        )?;
                        let filled = queued_exempt.filter(|&index| trades[index].open);
                        if let (Some(index), Some(callback)) =
                            (filled, callbacks.entry_filled.as_mut())
                        {
                            if callback(bar, &snapshot(&trades))? {
                                // The close now takes the new trade too, at the next bar's open.
                                queued_exempt = None;
                                if queued.exits.is_empty() {
                                    queued.exits.push(QueuedExit {
                                        key: PositionKey(index as u64),
                                        rule: None,
                                    });
                                }
                            }
                        }
                    }
                }

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
                        None,
                    );
                    queued = Decision::default();
                    queued_exempt = None;
                    working = None;
                    working_timed_exit = None;
                }
            }
        } else {
            strategy_signals(&mut callbacks, bar, &trades, inputs)?;
            entry_signals(&mut callbacks, bar, &trades, inputs)?;
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
        rejected,
    })
}

/// Between sections C and D: stops, targets and a resting fill resolve in one scan of the bar's
/// trade prices, so the first executable event in price order wins.
///
/// The range screen decides whether the bar's prices are asked for at all. A trade whose levels
/// the range cannot reach never fills here, and neither does a working order the range cannot
/// reach. A trade opened on this bar skips its own fill price. At each price a protective fill
/// takes precedence over a resting fill.
#[allow(
    clippy::too_many_arguments,
    reason = "the scan reads the bar's state, its costs and the working order"
)]
fn intrabar_fills(
    trades: &mut Vec<Trade>,
    working: &mut Option<WorkingGroup>,
    working_timed_exit: &mut Option<WorkingTimedExit>,
    rejected: &mut RejectedEntries,
    bar: usize,
    inputs: &CandleInputs<'_>,
    config: &CandleConfig,
    capital: &mut f64,
) -> Result<Option<usize>, CandleError> {
    let (Some(source), Some(high), Some(low)) = (inputs.intrabar, inputs.high, inputs.low) else {
        return Ok(None);
    };
    let (high, low) = (high[bar], low[bar]);
    let any_open = trades.iter().any(|trade| trade.open);
    if !any_open && working.is_none() {
        return Ok(None);
    }

    let mut reached: Vec<bool> = trades
        .iter()
        .map(|trade| trade.open && reached_by_range(trade, high, low, config))
        .collect();
    let order_reached = working
        .as_ref()
        .is_some_and(|group| group.reached_by_range(high, low));
    let timed_exit_active = working_timed_exit.is_some() && any_open;
    if !order_reached && !reached.contains(&true) && !timed_exit_active {
        return Ok(None);
    }

    let prices = intrabar_prices(source, bar)?;

    let mut open_trades: Vec<Trade> = trades.iter().filter(|trade| trade.open).cloned().collect();
    let mut intrabar_exit_tick: Option<usize> = None;
    if !open_trades.is_empty() {
        for (tick, (&time_us, &price)) in prices.time_us.iter().zip(&prices.price).enumerate() {
            let timed_exit_fires = working_timed_exit.is_some_and(|exit| {
                time_us >= exit.target_time_us && time_us >= exit.earliest_fill_us
            });
            let mut any_exited = false;
            for trade in open_trades.iter_mut() {
                if trade.open
                    && !trade.skips_tick(bar, tick)
                    && (trigger_price(trade, price, config).is_some() || timed_exit_fires)
                {
                    trade.open = false;
                    any_exited = true;
                }
            }
            if any_exited {
                intrabar_exit_tick = Some(tick);
            }
        }
    }

    let mut last_exit_tick = None;
    for (tick, (&time_us, &price)) in prices.time_us.iter().zip(&prices.price).enumerate() {
        for (index, trade) in trades.iter_mut().enumerate() {
            if !trade.open || !reached[index] || trade.skips_tick(bar, tick) {
                continue;
            }
            let Some((reason, fill)) = trigger_price(trade, price, config) else {
                continue;
            };
            let pnl = close_trade(trade, bar, fill, reason, config);
            trade.exit_time_us = time_us;
            *capital += pnl;
            last_exit_tick = Some(tick);
        }

        let timed_exit_fires = working_timed_exit
            .is_some_and(|exit| time_us >= exit.target_time_us && time_us >= exit.earliest_fill_us);
        if timed_exit_fires && trades.iter().any(|trade| trade.open) {
            working_timed_exit.take();
            for trade in trades.iter_mut() {
                if !trade.open || trade.skips_tick(bar, tick) {
                    continue;
                }
                let pnl = close_trade(trade, bar, price, ExitReason::Signal, config);
                trade.exit_time_us = time_us;
                *capital += pnl;
                last_exit_tick = Some(tick);
            }
        }

        let open_now = trades.iter().any(|trade| trade.open);
        if !open_now && !(order_reached && working.is_some()) {
            break;
        }

        // Q-132 rule: on a bar where a trade closed intrabar, resting orders / group members
        // fill only from a trade at or after that exit.
        let allowed_by_exit = intrabar_exit_tick.is_none_or(|exit_tick| tick >= exit_tick);
        if order_reached && allowed_by_exit {
            if let Some(group) = working.as_mut() {
                group.members.retain(|m| !m.expired_at(time_us));
                if group.members.is_empty() {
                    *working = None;
                } else {
                    let filled = group.members.iter().find_map(|member| {
                        if time_us < member.earliest_fill_us {
                            return None;
                        }
                        member.fill_at(price).map(|fill| (*member, fill))
                    });
                    if let Some((member, fill)) = filled {
                        let _ = working.take();
                        let kind = match member.order.kind {
                            RestingKind::Stop => EntryKind::Stop,
                            RestingKind::Limit => EntryKind::Limit,
                        };
                        let opening = Opening {
                            kind,
                            levels: member.levels,
                            time_us,
                            tick,
                        };
                        let entry = QueuedEntry {
                            side: member.side,
                            strength: member.strength,
                        };
                        if let Some(index) = open_entry(
                            trades, rejected, bar, fill, entry, opening, inputs, config, *capital,
                        ) {
                            trades[index].entry_candidate = member.candidate_index;
                            reached.push(reached_by_range(&trades[index], high, low, config));
                        }
                    }
                }
            }
        }
    }
    Ok(last_exit_tick)
}

fn reached_by_range(trade: &Trade, high: f64, low: f64, config: &CandleConfig) -> bool {
    trade.levels.reached_by_range(trade.side, high, low)
        || trade.breakeven.reached_by_range(
            trade.side,
            trade.entry_price,
            high,
            low,
            &config.exit_params,
        )
}

fn trigger_price(
    trade: &mut Trade,
    price: f64,
    config: &CandleConfig,
) -> Option<(ExitReason, f64)> {
    // Explicit protective orders retain precedence when both stops fire on the same print.
    trade.levels.trigger(trade.side, price).or_else(|| {
        trade
            .breakeven
            .on_price(trade.side, trade.entry_price, price, &config.exit_params)
            .map(|emergency| {
                (
                    if emergency {
                        ExitReason::BreakevenEmergency
                    } else {
                        ExitReason::Rule(ExitRuleId::Breakeven)
                    },
                    price,
                )
            })
    })
}

/// A bar's trade prices from the source. The source must return a price and one time per price.
fn intrabar_prices<E: From<CandleError>>(
    source: &dyn IntrabarSource,
    bar: usize,
) -> Result<IntrabarPrices, E> {
    let source_error = |reason: &str| CandleError::IntrabarSource {
        bar,
        reason: reason.to_string(),
    };
    let prices = source.prices(bar).map_err(|reason| source_error(&reason))?;
    if prices.price.is_empty() {
        return Err(source_error("returned no prices").into());
    }
    if prices.time_us.len() != prices.price.len() {
        return Err(source_error("time_us and price differ in length").into());
    }
    Ok(prices)
}

/// A priced entry candidate with its levels resolved.
#[derive(Clone, Copy, Debug)]
struct PricedCandidate {
    side: Side,
    price: f64,
    levels: Levels,
}

/// The priced entries offered on `bar`, empty when it has none and its entry queues for the next
/// open. Runtime candidates replace the static `entry_price` column; each price must lie within the
/// bar's range.
fn priced_entry(
    inputs: &CandleInputs<'_>,
    bar: usize,
    side: Side,
    runtime: Option<&[EntryCandidate]>,
    runtime_levels: Option<(f64, f64)>,
    levels: Levels,
) -> Result<Vec<PricedCandidate>, CandleError> {
    let candidates: Vec<PricedCandidate> = match runtime {
        Some(candidates) => candidates
            .iter()
            .map(|candidate| {
                let levels = match candidate.levels.or(runtime_levels) {
                    Some(own) => entry_levels(inputs, bar, Some(own))?,
                    None => levels,
                };
                Ok(PricedCandidate {
                    side: candidate.side,
                    price: candidate.price,
                    levels,
                })
            })
            .collect::<Result<_, CandleError>>()?,
        None => {
            let price = inputs
                .protective
                .and_then(|columns| columns.entry_price)
                .map_or(f64::NAN, |prices| prices[bar]);
            if price.is_nan() {
                Vec::new()
            } else {
                vec![PricedCandidate {
                    side,
                    price,
                    levels,
                }]
            }
        }
    };
    if candidates.is_empty() {
        return Ok(candidates);
    }
    let (Some(_), Some(high), Some(low)) = (inputs.open, inputs.high, inputs.low) else {
        return Err(CandleError::InvalidConfig {
            field: "entry_price",
            reason: "requires open, high and low".to_string(),
        });
    };
    for candidate in &candidates {
        // Written as the in-range test so a NaN bar bound rejects the price.
        let inside = low[bar] <= candidate.price && candidate.price <= high[bar];
        if !inside {
            return Err(CandleError::EntryPriceOutsideRange {
                bar,
                side: candidate.side,
                price: candidate.price,
                low: low[bar],
                high: high[bar],
            });
        }
    }
    Ok(candidates)
}

/// Section D's priced entry: the candidate the tape fills first, the earlier listed one on a tie, is
/// sized and capped like any entry, then opened on this bar and resolved from the trade prices after
/// its fill. A candidate fills as [`priced_fill`] fills a stop or a limit, from the bar's first trade
/// or, after a same-bar exit, from the latest exit's trade. The others are cancelled. When none
/// fills, every candidate is refused: with reason `1` after an exit, `2` otherwise. Without an
/// intrabar source a single candidate fills at its own price.
#[allow(
    clippy::too_many_arguments,
    reason = "a priced entry reads the bar's state, its costs, its candidates and the rejection log"
)]
fn open_priced_entry<E: From<CandleError>>(
    trades: &mut Vec<Trade>,
    bar: usize,
    entry: QueuedEntry,
    candidates: &[PricedCandidate],
    last_exit_tick: Option<usize>,
    inputs: &CandleInputs<'_>,
    config: &CandleConfig,
    capital: &mut f64,
    rejected: &mut RejectedEntries,
) -> Result<Option<usize>, E> {
    let tape = match inputs.intrabar {
        Some(source) => Some(intrabar_prices::<E>(source, bar)?),
        None => None,
    };
    let (index, fill, touch) = match &tape {
        Some(prices) => {
            let start = last_exit_tick.unwrap_or(0);
            let first = candidates
                .iter()
                .enumerate()
                .filter_map(|(index, candidate)| {
                    let buy = candidate.side == Side::Long;
                    priced_fill(&prices.price, candidate.price, buy, start)
                        .map(|(tick, fill)| (tick, index, fill))
                })
                .min_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
            let Some((tick, index, fill)) = first else {
                let reason = if last_exit_tick.is_some() { 1 } else { 2 };
                for candidate in candidates {
                    rejected.push(
                        bar,
                        candidate.side,
                        candidate.price,
                        candidate.levels,
                        reason,
                    );
                }
                return Ok(None);
            };
            (index, fill, Some(tick))
        }
        None if candidates.len() == 1 => (0, candidates[0].price, None),
        None => {
            return Err(CandleError::InvalidConfig {
                field: "entry_price",
                reason: "several priced entries require intrabar, open, high and low".to_string(),
            }
            .into())
        }
    };
    let chosen = candidates[index];
    let opening = Opening {
        kind: EntryKind::Priced,
        levels: chosen.levels,
        time_us: -1,
        tick: touch.unwrap_or(0),
    };
    let entry = QueuedEntry {
        side: chosen.side,
        ..entry
    };
    let Some(opened) = open_entry(
        trades, rejected, bar, fill, entry, opening, inputs, config, *capital,
    ) else {
        return Ok(None);
    };
    trades[opened].entry_candidate = index as i64;
    let filled = touch.zip(tape.as_ref());
    resolve_entry_levels(&mut trades[opened], bar, inputs, config, capital, filled)?;
    Ok(Some(opened))
}

/// Resolves a priced entry's own stop and target from the bar's trade prices after its touch. The
/// source is asked only when the bar's range reaches one of the levels. The first price after the
/// touch that triggers a level closes the trade; the touch itself is the entry and is skipped.
fn resolve_entry_levels<E: From<CandleError>>(
    trade: &mut Trade,
    bar: usize,
    inputs: &CandleInputs<'_>,
    config: &CandleConfig,
    capital: &mut f64,
    filled: Option<(usize, &IntrabarPrices)>,
) -> Result<(), E> {
    let (Some((touch, prices)), Some(high), Some(low)) = (filled, inputs.high, inputs.low) else {
        return Ok(());
    };
    if !reached_by_range(trade, high[bar], low[bar], config) {
        return Ok(());
    }

    let exit = prices
        .time_us
        .iter()
        .zip(&prices.price)
        .skip(touch + 1)
        .find_map(|(&time_us, &price)| {
            trigger_price(trade, price, config).map(|(reason, fill)| (time_us, reason, fill))
        });
    if let Some((time_us, reason, fill)) = exit {
        let pnl = close_trade(trade, bar, fill, reason, config);
        trade.exit_time_us = time_us;
        *capital += pnl;
    }
    Ok(())
}

/// Levels for an entry queued on `bar`: the runtime pair when the strategy returned one, else the
/// static columns. Runtime levels need the price source, as static ones do.
fn entry_levels<E: From<CandleError>>(
    inputs: &CandleInputs<'_>,
    bar: usize,
    runtime: Option<(f64, f64)>,
) -> Result<Levels, E> {
    let Some((stop, target)) = runtime else {
        return Ok(Levels::queued_on(inputs.protective, bar));
    };
    if (!stop.is_nan() || !target.is_nan()) && !intrabar_ready(inputs) {
        return Err(CandleError::InvalidConfig {
            field: "stop_price",
            reason: "runtime levels require intrabar, open, high and low".to_string(),
        }
        .into());
    }
    Ok(Levels { stop, target })
}

/// How an entry opens: its kind and levels, the time of its fill (`-1` for every kind but a resting
/// order), and the scan tick it fills on (`0` for entries opened before the scan).
#[derive(Clone, Copy, Debug)]
struct Opening {
    kind: EntryKind,
    levels: Levels,
    time_us: i64,
    tick: usize,
}

/// Sizes an entry at its fill price with the current capital, then trims it to the position cap.
/// An entry the cap leaves no room for is skipped, not rejected. Otherwise an entry whose levels
/// sit on the wrong side of its fill is refused and reported. Returns the new trade's index.
///
/// An entry the sizing rule refuses is still reported when its levels are on the wrong side, as
/// it always was.
#[allow(
    clippy::too_many_arguments,
    reason = "an entry reads the bar's state, the fill, its opening kind and the rejection log"
)]
fn open_entry(
    trades: &mut Vec<Trade>,
    rejected: &mut RejectedEntries,
    bar: usize,
    fill: f64,
    entry: QueuedEntry,
    opening: Opening,
    inputs: &CandleInputs<'_>,
    config: &CandleConfig,
    capital: f64,
) -> Option<usize> {
    let volatility = inputs.volatility.map(|column| column[bar]);
    let Some(mut quantity) = config
        .sizing
        .size(entry.strength, fill, capital, volatility)
    else {
        if opening.levels.rejects(entry.side, fill) {
            rejected.push(bar, entry.side, fill, opening.levels, 0);
        }
        return None;
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
            return None;
        }
        if quantity > remaining {
            quantity = remaining;
        }
    }

    if opening.levels.rejects(entry.side, fill) {
        rejected.push(bar, entry.side, fill, opening.levels, 0);
        return None;
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
        levels: opening.levels,
        exit_time_us: -1,
        breakeven: BreakevenStop::default(),
        kind: opening.kind,
        entry_time_us: opening.time_us,
        first_tick: opening.tick,
        entry_candidate: -1,
        exit_candidate: -1,
    });
    Some(trades.len() - 1)
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
    runtime_signals: Option<BarSignals>,
) -> Decision {
    let Some(window) = config.day_trade else {
        return decide(
            step,
            bar,
            trades,
            inputs,
            exit_inputs,
            config,
            runtime_signals,
        );
    };

    if now >= window.force_close_us || last_bar_of_day {
        return Decision::default();
    }

    let mut decision = decide(
        step,
        bar,
        trades,
        inputs,
        exit_inputs,
        config,
        runtime_signals,
    );
    if !entry_allowed(config, now, last_bar_of_day) {
        decision.entry = None;
    }
    decision
}

/// The day-trade gate on an entry decided at `now`: inside the entry window and before the
/// session's force-close and last bar.
fn entry_allowed(config: &CandleConfig, now: i64, last_bar_of_day: bool) -> bool {
    config.day_trade.is_none_or(|window| {
        now < window.force_close_us
            && !last_bar_of_day
            && now >= window.entry_start_us
            && now <= window.entry_end_us
    })
}

fn decide(
    step: &mut DecisionStep,
    bar: usize,
    trades: &[Trade],
    inputs: &CandleInputs<'_>,
    exit_inputs: &ExitInputs<'_>,
    config: &CandleConfig,
    runtime_signals: Option<BarSignals>,
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
    step.decide_bar(
        bar,
        &open,
        runtime_signals.unwrap_or_else(|| BarSignals::at(&inputs.signals, bar)),
        inputs.signals.bar_index,
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
        ledger.exit_time_us.push(trade.exit_time_us);
        ledger.entry_kind.push(trade.kind.code());
        ledger.entry_time_us.push(trade.entry_time_us);
        ledger.final_stop_price.push(trade.levels.stop);
        ledger.final_target_price.push(trade.levels.target);
        ledger.entry_candidate.push(trade.entry_candidate);
        ledger.exit_candidate.push(trade.exit_candidate);
    }
    ledger
}

/// Applies a decision's amendments to every open trade. The callback has just seen those trades,
/// and the levels first matter on the next bar's scan.
fn amend_levels(trades: &mut [Trade], signals: &BarSignals) {
    for trade in trades.iter_mut().filter(|trade| trade.open) {
        if let Some(stop) = signals.amend_stop {
            trade.levels.stop = stop;
        }
        if let Some(target) = signals.amend_target {
            trade.levels.target = target;
        }
    }
}

/// A resting order needs an entry decision and a positive price, and `expire_bars` is positive.
fn check_resting(signals: &BarSignals, order: RestingOrder, bar: usize) -> Result<(), CandleError> {
    if signals.entry == 0 {
        return Err(CandleError::InvalidSignal {
            column: "entry",
            bar,
            reason: "a resting order needs an entry decision",
        });
    }
    check_positive("entry_price", bar, order.price)?;
    if order.expire_bars == Some(0) {
        return Err(CandleError::InvalidSignal {
            column: "expire_bars",
            bar,
            reason: "must be a positive integer",
        });
    }
    if let Some(exp) = order.expire_time_us {
        check_positive("expire_time_us", bar, exp as f64)?;
    }
    Ok(())
}

fn check_positive(column: &'static str, bar: usize, value: f64) -> Result<(), CandleError> {
    if value.is_finite() && value > 0.0 {
        return Ok(());
    }
    Err(CandleError::InvalidSignal {
        column,
        bar,
        reason: "must be finite and positive",
    })
}

fn check_level(column: &'static str, bar: usize, level: f64) -> Result<(), CandleError> {
    if level.is_nan() || (level.is_finite() && level > 0.0) {
        return Ok(());
    }
    Err(CandleError::InvalidSignal {
        column,
        bar,
        reason: "must be NaN or a finite positive price",
    })
}

fn validate(inputs: &CandleInputs<'_>, config: &CandleConfig) -> Result<(), CandleError> {
    config.sizing.validate()?;
    if config.latency_us < 0 {
        return Err(CandleError::InvalidConfig {
            field: "latency_us",
            reason: "must be non-negative".to_string(),
        });
    }
    let params = &config.exit_params;
    if params.intrabar_breakeven() {
        if !intrabar_ready(inputs) {
            return Err(CandleError::InvalidConfig {
                field: "breakeven",
                reason: "requires intrabar, open, high and low".to_string(),
            });
        }
        if !params.breakeven_trigger_pct.is_finite()
            || !params.breakeven_offset_pct.is_finite()
            || params.breakeven_offset_pct < 0.0
            || params.breakeven_offset_pct >= params.breakeven_trigger_pct
        {
            return Err(CandleError::InvalidConfig {
                field: "breakeven_offset_pct",
                reason: "must be finite, nonnegative and below the finite activation threshold"
                    .to_string(),
            });
        }
        if params.breakeven_emergency_stop_ticks.unwrap_or(0) > 0
            && (!params.tick_size.is_finite()
                || params.tick_size <= 0.0
                || !(params.breakeven_emergency_stop_ticks.unwrap_or(0) as f64 * params.tick_size)
                    .is_finite())
        {
            return Err(CandleError::InvalidConfig {
                field: "tick_size",
                reason:
                    "emergency stop requires a finite positive instrument tick size and distance"
                        .to_string(),
            });
        }
    }

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
    if let Some(protective) = inputs.protective {
        let bar_range = inputs.open.is_some() && inputs.high.is_some() && inputs.low.is_some();
        if protective.entry_price.is_some() && !bar_range {
            return Err(CandleError::InvalidConfig {
                field: "entry_price",
                reason: "requires open, high and low".to_string(),
            });
        }
        // Without an entry price the columns are levels alone. Levels need the intrabar source;
        // a column of only NaN levels beside a priced entry does not.
        let levels_set = protective
            .stop_price
            .iter()
            .chain(protective.target_price)
            .any(|level| !level.is_nan());
        if (protective.entry_price.is_none() || levels_set)
            && (inputs.intrabar.is_none() || !bar_range)
        {
            return Err(CandleError::InvalidConfig {
                field: "protective",
                reason: "requires intrabar, open, high and low".to_string(),
            });
        }
        check("stop_price", protective.stop_price.len())?;
        check("target_price", protective.target_price.len())?;
        if let Some(entry_price) = protective.entry_price {
            check("entry_price", entry_price.len())?;
        }
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

    if let Some(protective) = inputs.protective {
        for (column, levels) in [
            ("stop_price", protective.stop_price),
            ("target_price", protective.target_price),
            ("entry_price", protective.entry_price.unwrap_or(&[])),
        ] {
            for (bar, &level) in levels.iter().enumerate() {
                check_level(column, bar, level)?;
            }
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
                protective: None,
                intrabar: None,
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
            latency_us: 0,
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
