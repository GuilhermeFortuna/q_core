//! Python projection of `q-engine::execution`: stateless quote and trade matching and fill costs.
//!
//! Enum arguments are lowercase strings. Decimal arguments are exact decimal strings, never floats.
//! Invalid values raise `ValueError`; an integer or result outside the native range raises
//! `OverflowError`. Prices and quantities are integers in ticks and volume steps.

use pyo3::exceptions::{PyOverflowError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyModule;
use pyo3::Bound;
use q_engine::candle::RestingKind;
use q_engine::execution::{self, ExecutionError, FillCost, OrderKind, OrderSide, Quote};
use rust_decimal::Decimal;

/// Capability version of this API, advertised as `q_core.execution.INCREMENTAL_EXECUTION`.
const INCREMENTAL_EXECUTION: u32 = 1;

fn execution_error(err: &ExecutionError) -> PyErr {
    match err {
        ExecutionError::Invalid { .. } => PyValueError::new_err(err.to_string()),
        ExecutionError::Overflow { .. } => PyOverflowError::new_err(err.to_string()),
    }
}

fn parse_side(side: &str) -> PyResult<OrderSide> {
    match side {
        "buy" => Ok(OrderSide::Buy),
        "sell" => Ok(OrderSide::Sell),
        other => Err(PyValueError::new_err(format!(
            "side must be 'buy' or 'sell', got {other:?}"
        ))),
    }
}

fn parse_kind(kind: &str) -> PyResult<OrderKind> {
    match kind {
        "market" => Ok(OrderKind::Market),
        "stop" => Ok(OrderKind::Stop),
        "limit" => Ok(OrderKind::Limit),
        other => Err(PyValueError::new_err(format!(
            "kind must be 'market', 'stop' or 'limit', got {other:?}"
        ))),
    }
}

fn parse_resting_kind(kind: &str) -> PyResult<RestingKind> {
    match kind {
        "stop" => Ok(RestingKind::Stop),
        "limit" => Ok(RestingKind::Limit),
        other => Err(PyValueError::new_err(format!(
            "kind must be 'stop' or 'limit' for match_trade, got {other:?}"
        ))),
    }
}

fn parse_decimal(field: &str, text: &str) -> PyResult<Decimal> {
    Decimal::from_str_exact(text).map_err(|_| {
        PyValueError::new_err(format!("{field}: not an exact decimal string: {text:?}"))
    })
}

/// Quote-model fill of one order. Returns the fill in ticks, or `None` if the order does not fill.
#[pyfunction]
#[pyo3(signature = (*, side, kind, level_ticks, bid_ticks, ask_ticks, slippage_ticks = 0))]
fn match_quote(
    side: &str,
    kind: &str,
    level_ticks: Option<i64>,
    bid_ticks: i64,
    ask_ticks: i64,
    slippage_ticks: i64,
) -> PyResult<Option<i64>> {
    let side = parse_side(side)?;
    let kind = parse_kind(kind)?;
    let quote = Quote {
        bid: bid_ticks,
        ask: ask_ticks,
    };
    execution::match_quote(side, kind, level_ticks, quote, slippage_ticks)
        .map_err(|e| execution_error(&e))
}

/// Trade-model fill of a resting stop or limit against one trade. Returns the fill in ticks, or `None`.
#[pyfunction]
#[pyo3(signature = (*, side, kind, level_ticks, trade_ticks))]
fn match_trade(
    side: &str,
    kind: &str,
    level_ticks: i64,
    trade_ticks: i64,
) -> PyResult<Option<i64>> {
    let side = parse_side(side)?;
    let kind = parse_resting_kind(kind)?;
    execution::match_trade(side, kind, level_ticks, trade_ticks).map_err(|e| execution_error(&e))
}

/// Currency fee of one fill as an exact decimal string with `currency_decimals` places.
#[pyfunction]
#[pyo3(signature = (*, quantity_steps, volume_step, price_ticks, tick_size, point_value, cost_per_contract, cost_bps, currency_decimals))]
#[allow(
    clippy::too_many_arguments,
    reason = "keyword-only Python signature mirrors the documented fee inputs"
)]
fn fill_cost(
    quantity_steps: i64,
    volume_step: &str,
    price_ticks: i64,
    tick_size: &str,
    point_value: &str,
    cost_per_contract: &str,
    cost_bps: &str,
    currency_decimals: i64,
) -> PyResult<String> {
    let currency_decimals = u32::try_from(currency_decimals).map_err(|_| {
        PyValueError::new_err("currency_decimals: must be nonnegative and at most 28")
    })?;
    let cost = FillCost {
        quantity_steps,
        volume_step: parse_decimal("volume_step", volume_step)?,
        price_ticks,
        tick_size: parse_decimal("tick_size", tick_size)?,
        point_value: parse_decimal("point_value", point_value)?,
        cost_per_contract: parse_decimal("cost_per_contract", cost_per_contract)?,
        cost_bps: parse_decimal("cost_bps", cost_bps)?,
        currency_decimals,
    };
    execution::fill_cost(&cost)
        .map(|fee| fee.to_string())
        .map_err(|e| execution_error(&e))
}

/// Converts an exact decimal price to integer ticks, rejecting prices off the tick grid.
#[pyfunction]
#[pyo3(signature = (*, price, tick_size))]
fn price_to_ticks(price: &str, tick_size: &str) -> PyResult<i64> {
    let price = parse_decimal("price", price)?;
    let tick_size = parse_decimal("tick_size", tick_size)?;
    execution::price_to_ticks(price, tick_size).map_err(|e| execution_error(&e))
}

pub(crate) fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let m = PyModule::new(parent.py(), "execution")?;
    m.add("INCREMENTAL_EXECUTION", INCREMENTAL_EXECUTION)?;
    m.add_function(wrap_pyfunction!(match_quote, &m)?)?;
    m.add_function(wrap_pyfunction!(match_trade, &m)?)?;
    m.add_function(wrap_pyfunction!(fill_cost, &m)?)?;
    m.add_function(wrap_pyfunction!(price_to_ticks, &m)?)?;
    parent.add_submodule(&m)?;
    parent
        .py()
        .import("sys")?
        .getattr("modules")?
        .set_item("q_core.execution", &m)?;
    Ok(())
}
