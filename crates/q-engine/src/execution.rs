//! Stateless execution primitives for one order against one observed market event.
//!
//! A continuously running simulated broker calls these functions for one order and one event. They
//! hold no orders, accounts, queues or clocks; the caller owns that state.
//!
//! Two fill models are explicit and deliberately not interchangeable:
//!
//! - **Quote** ([`match_quote`]): buys trade at the ask and sells at the bid. A stop triggers at or
//!   beyond its level and fills as a market order, with adverse slippage. A limit fills at the current
//!   executable quote, which may improve on its level; slippage never applies to a limit.
//! - **Trade** ([`match_trade`]): the candle resting-order rule on the last trade. A stop fills at the
//!   first trade at or beyond its level; a limit fills at its level once a trade prints strictly beyond
//!   it. An equal print does not fill a limit.
//!
//! Prices are integer ticks and quantities integer volume steps. Decimal metadata is converted at the
//! Python boundary. [`fill_cost`] returns the currency fee of one fill, exact and rounded once.

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};

use crate::candle::RestingKind;

/// Side of an order. A buy trades at the ask and a sell at the bid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

/// Kind of an order matched against a quote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderKind {
    /// Fills at once at the executable quote, plus adverse slippage. Takes no level.
    Market,
    /// Triggers at or beyond its level; fills as a market order.
    Stop,
    /// Fills at the executable quote once it reaches its level, possibly improved.
    Limit,
}

/// A two-sided quote in integer price ticks. Valid when `0 < bid <= ask`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quote {
    pub bid: i64,
    pub ask: i64,
}

/// Inputs for [`fill_cost`].
///
/// Quantity is `quantity_steps * volume_step` in contracts, and price is `price_ticks * tick_size` in
/// price units. The fee is `quantity * cost_per_contract + |quantity * price * point_value| * cost_bps / 10000`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FillCost {
    /// Non-negative count of volume steps.
    pub quantity_steps: i64,
    /// Positive contract size of one volume step.
    pub volume_step: Decimal,
    /// Positive fill price in ticks.
    pub price_ticks: i64,
    /// Positive price increment.
    pub tick_size: Decimal,
    /// Positive currency value of one price unit per contract.
    pub point_value: Decimal,
    /// Non-negative currency fee per contract.
    pub cost_per_contract: Decimal,
    /// Non-negative fee in basis points of notional.
    pub cost_bps: Decimal,
    /// Decimal places of the returned currency amount, at most [`Decimal::MAX_SCALE`].
    pub currency_decimals: u32,
}

/// Errors from the execution primitives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionError {
    /// An input or result violates a documented rule. The field names the input.
    Invalid {
        field: &'static str,
        reason: &'static str,
    },
    /// An exact result does not fit the native integer or decimal representation.
    Overflow { field: &'static str },
}

impl std::fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid { field, reason } => write!(f, "{field}: {reason}"),
            Self::Overflow { field } => write!(f, "{field}: exact result overflows"),
        }
    }
}

impl std::error::Error for ExecutionError {}

/// Quote-model fill of one order against `quote`.
///
/// `level` is the stop or limit price in ticks; it must be absent for a market order and present
/// otherwise. Returns `Ok(None)` when the order does not fill on this quote.
///
/// # Errors
///
/// Returns [`ExecutionError::Invalid`] for a crossed or nonpositive quote, a negative slippage, a
/// level that does not match the kind, a nonpositive level, or a sell fill that slippage would make
/// nonpositive. Returns [`ExecutionError::Overflow`] when adverse slippage overflows `i64`.
pub fn match_quote(
    side: OrderSide,
    kind: OrderKind,
    level: Option<i64>,
    quote: Quote,
    slippage: i64,
) -> Result<Option<i64>, ExecutionError> {
    validate_quote(quote)?;
    if slippage < 0 {
        return Err(invalid("slippage_ticks", "must be nonnegative"));
    }
    let touch = quote_price(side, quote.bid, quote.ask);
    match kind {
        OrderKind::Market => {
            if level.is_some() {
                return Err(invalid("level_ticks", "a market order takes no level"));
            }
            adverse_fill(side, touch, slippage).map(Some)
        }
        OrderKind::Stop => {
            let level = required_level(level)?;
            if quote_reaches(side, RestingKind::Stop, level, touch) {
                adverse_fill(side, touch, slippage).map(Some)
            } else {
                Ok(None)
            }
        }
        OrderKind::Limit => {
            let level = required_level(level)?;
            // The executable quote is at or through the level, so the fill is at least as good.
            Ok(quote_reaches(side, RestingKind::Limit, level, touch).then_some(touch))
        }
    }
}

/// Trade-model fill of a resting stop or limit against one observed trade.
///
/// A stop fills at the trade when it is at or beyond `level`; a limit fills at `level` once the
/// trade is strictly beyond it. The same comparisons as the candle resting-order rule.
///
/// # Errors
///
/// Returns [`ExecutionError::Invalid`] when `level` or `trade` is nonpositive.
pub fn match_trade(
    side: OrderSide,
    kind: RestingKind,
    level: i64,
    trade: i64,
) -> Result<Option<i64>, ExecutionError> {
    positive_i64("level_ticks", level)?;
    positive_i64("trade_ticks", trade)?;
    Ok(trade_fill(side, kind, level, trade))
}

/// Currency fee of one fill, rounded half-to-even once at `currency_decimals` places.
///
/// Every intermediate product is exact. A product or sum that Decimal cannot represent exactly
/// (its scale or 96-bit mantissa would be exceeded) is an overflow, never a silent rounding.
///
/// # Errors
///
/// Returns [`ExecutionError::Invalid`] for a negative size, cost or rate, a nonpositive step, tick
/// size, point value or price, or `currency_decimals` above [`Decimal::MAX_SCALE`]. Returns
/// [`ExecutionError::Overflow`] when an exact intermediate result is not representable.
pub fn fill_cost(cost: &FillCost) -> Result<Decimal, ExecutionError> {
    if cost.quantity_steps < 0 {
        return Err(invalid("quantity_steps", "must be nonnegative"));
    }
    positive_decimal("volume_step", cost.volume_step)?;
    positive_i64("price_ticks", cost.price_ticks)?;
    positive_decimal("tick_size", cost.tick_size)?;
    positive_decimal("point_value", cost.point_value)?;
    nonnegative_decimal("cost_per_contract", cost.cost_per_contract)?;
    nonnegative_decimal("cost_bps", cost.cost_bps)?;
    if cost.currency_decimals > Decimal::MAX_SCALE {
        return Err(invalid(
            "currency_decimals",
            "exceeds the decimal scale limit of 28",
        ));
    }

    let quantity = exact_mul(
        "quantity",
        Decimal::from(cost.quantity_steps),
        cost.volume_step,
    )?;
    let price = exact_mul("price", Decimal::from(cost.price_ticks), cost.tick_size)?;
    let notional = exact_mul(
        "notional",
        exact_mul("notional", quantity, price)?,
        cost.point_value,
    )?
    .abs();
    let per_contract = exact_mul("per_contract_fee", quantity, cost.cost_per_contract)?;
    let basis_points = exact_mul(
        "bps_fee",
        exact_mul("bps_fee", notional, cost.cost_bps)?,
        Decimal::new(1, 4),
    )?;
    let total = exact_add("fill_cost", per_contract, basis_points)?;

    let mut rounded = total.round_dp_with_strategy(
        cost.currency_decimals,
        RoundingStrategy::MidpointNearestEven,
    );
    rounded.rescale(cost.currency_decimals);
    Ok(rounded)
}

/// Converts a price to integer ticks, rejecting prices that are not on the tick grid.
///
/// # Errors
///
/// Returns [`ExecutionError::Invalid`] for a nonpositive price or tick size, or a price that is not
/// an exact multiple of `tick_size`. Returns [`ExecutionError::Overflow`] when the tick count does
/// not fit `i64`.
pub fn price_to_ticks(price: Decimal, tick_size: Decimal) -> Result<i64, ExecutionError> {
    positive_decimal("price", price)?;
    positive_decimal("tick_size", tick_size)?;
    let remainder = price
        .checked_rem(tick_size)
        .ok_or(ExecutionError::Overflow { field: "price" })?;
    if !remainder.is_zero() {
        return Err(invalid("price", "is not a multiple of tick_size"));
    }
    let ticks = price
        .checked_div(tick_size)
        .ok_or(ExecutionError::Overflow { field: "price" })?;
    ticks
        .to_i64()
        .ok_or(ExecutionError::Overflow { field: "price" })
}

/// The quote an order of `side` trades against: the ask for a buy, the bid for a sell.
pub(crate) fn quote_price<T: Copy>(side: OrderSide, bid: T, ask: T) -> T {
    match side {
        OrderSide::Buy => ask,
        OrderSide::Sell => bid,
    }
}

/// Whether a quote reaches a resting order: a buy stop at or above its level, a buy limit at or
/// below, a sell stop at or below, and a sell limit at or above. Shared by the quote model and the
/// tick kernel's exits.
pub(crate) fn quote_reaches<T: PartialOrd + Copy>(
    side: OrderSide,
    kind: RestingKind,
    level: T,
    quote: T,
) -> bool {
    match (side, kind) {
        (OrderSide::Buy, RestingKind::Stop) => quote >= level,
        (OrderSide::Buy, RestingKind::Limit) => quote <= level,
        (OrderSide::Sell, RestingKind::Stop) => quote <= level,
        (OrderSide::Sell, RestingKind::Limit) => quote >= level,
    }
}

/// Trade-model fill of a resting order on one trade price, shared with the candle kernel.
///
/// A stop fills at the trade price at or beyond its level. A limit fills at its level once a trade
/// goes strictly beyond it. Generic so that candle prices (`f64`) and integer ticks share one rule.
pub(crate) fn trade_fill<T: PartialOrd + Copy>(
    side: OrderSide,
    kind: RestingKind,
    level: T,
    trade: T,
) -> Option<T> {
    match (side, kind) {
        (OrderSide::Buy, RestingKind::Stop) => (trade >= level).then_some(trade),
        (OrderSide::Buy, RestingKind::Limit) => (trade < level).then_some(level),
        (OrderSide::Sell, RestingKind::Stop) => (trade <= level).then_some(trade),
        (OrderSide::Sell, RestingKind::Limit) => (trade > level).then_some(level),
    }
}

fn adverse_fill(side: OrderSide, touch: i64, slippage: i64) -> Result<i64, ExecutionError> {
    match side {
        OrderSide::Buy => touch.checked_add(slippage).ok_or(ExecutionError::Overflow {
            field: "fill_ticks",
        }),
        // `touch` is positive and `slippage` nonnegative, so this subtraction cannot overflow.
        OrderSide::Sell => {
            let fill = touch - slippage;
            if fill > 0 {
                Ok(fill)
            } else {
                Err(invalid(
                    "fill_ticks",
                    "a sell fill after adverse slippage must stay positive",
                ))
            }
        }
    }
}

fn required_level(level: Option<i64>) -> Result<i64, ExecutionError> {
    let level = level.ok_or_else(|| invalid("level_ticks", "a stop or limit needs a level"))?;
    positive_i64("level_ticks", level)?;
    Ok(level)
}

fn validate_quote(quote: Quote) -> Result<(), ExecutionError> {
    positive_i64("bid_ticks", quote.bid)?;
    positive_i64("ask_ticks", quote.ask)?;
    if quote.bid > quote.ask {
        return Err(invalid("quote", "crossed: bid is above ask"));
    }
    Ok(())
}

fn exact_mul(field: &'static str, a: Decimal, b: Decimal) -> Result<Decimal, ExecutionError> {
    if a.is_zero() || b.is_zero() {
        return Ok(Decimal::ZERO);
    }
    let scale = a.scale() + b.scale();
    let product = a.checked_mul(b).ok_or(ExecutionError::Overflow { field })?;
    // Decimal multiplication rounds when the exact scale or mantissa does not fit; a shorter scale
    // reveals that rounding, so it is reported rather than accepted.
    if product.scale() != scale {
        return Err(ExecutionError::Overflow { field });
    }
    Ok(product)
}

fn exact_add(field: &'static str, a: Decimal, b: Decimal) -> Result<Decimal, ExecutionError> {
    if a.is_zero() {
        return Ok(b);
    }
    if b.is_zero() {
        return Ok(a);
    }
    let scale = a.scale().max(b.scale());
    let sum = a.checked_add(b).ok_or(ExecutionError::Overflow { field })?;
    if sum.scale() != scale {
        return Err(ExecutionError::Overflow { field });
    }
    Ok(sum)
}

fn positive_i64(field: &'static str, value: i64) -> Result<(), ExecutionError> {
    if value > 0 {
        Ok(())
    } else {
        Err(invalid(field, "must be positive"))
    }
}

fn positive_decimal(field: &'static str, value: Decimal) -> Result<(), ExecutionError> {
    if value.is_sign_positive() && !value.is_zero() {
        Ok(())
    } else {
        Err(invalid(field, "must be positive"))
    }
}

fn nonnegative_decimal(field: &'static str, value: Decimal) -> Result<(), ExecutionError> {
    if value.is_sign_negative() {
        Err(invalid(field, "must be nonnegative"))
    } else {
        Ok(())
    }
}

fn invalid(field: &'static str, reason: &'static str) -> ExecutionError {
    ExecutionError::Invalid { field, reason }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(text: &str) -> Decimal {
        Decimal::from_str_exact(text).unwrap()
    }

    #[test]
    fn quote_price_selects_ask_for_buys_and_bid_for_sells() {
        assert_eq!(quote_price(OrderSide::Buy, 99, 100), 100);
        assert_eq!(quote_price(OrderSide::Sell, 99, 100), 99);
    }

    #[test]
    fn fill_cost_rounds_half_to_even_once_at_currency_precision() {
        let base = FillCost {
            quantity_steps: 1,
            volume_step: dec("1"),
            price_ticks: 1,
            tick_size: dec("1"),
            point_value: dec("1"),
            cost_per_contract: dec("0.125"),
            cost_bps: dec("0"),
            currency_decimals: 2,
        };
        assert_eq!(fill_cost(&base).unwrap().to_string(), "0.12");
        let next = FillCost {
            cost_per_contract: dec("0.135"),
            ..base
        };
        assert_eq!(fill_cost(&next).unwrap().to_string(), "0.14");
    }

    #[test]
    fn fill_cost_pads_to_requested_places_and_rejects_bad_domains() {
        let zero = FillCost {
            quantity_steps: 0,
            volume_step: dec("0.01"),
            price_ticks: 1,
            tick_size: dec("0.5"),
            point_value: dec("1"),
            cost_per_contract: dec("1"),
            cost_bps: dec("1"),
            currency_decimals: 2,
        };
        assert_eq!(fill_cost(&zero).unwrap().to_string(), "0.00");
        assert!(matches!(
            fill_cost(&FillCost {
                quantity_steps: -1,
                ..zero.clone()
            }),
            Err(ExecutionError::Invalid {
                field: "quantity_steps",
                ..
            })
        ));
        assert!(matches!(
            fill_cost(&FillCost {
                tick_size: dec("0"),
                ..zero.clone()
            }),
            Err(ExecutionError::Invalid {
                field: "tick_size",
                ..
            })
        ));
        assert!(matches!(
            fill_cost(&FillCost {
                cost_bps: dec("-1"),
                ..zero.clone()
            }),
            Err(ExecutionError::Invalid {
                field: "cost_bps",
                ..
            })
        ));
        assert!(matches!(
            fill_cost(&FillCost {
                currency_decimals: 29,
                ..zero
            }),
            Err(ExecutionError::Invalid {
                field: "currency_decimals",
                ..
            })
        ));
    }

    #[test]
    fn fill_cost_reports_unrepresentable_products_instead_of_rounding() {
        // Quantity and price each carry 28 decimal places, so their product needs 56 and would round.
        let tiny = "0.0000000000000000000000000001";
        let deep = FillCost {
            quantity_steps: 1,
            volume_step: dec(tiny),
            price_ticks: 1,
            tick_size: dec(tiny),
            point_value: dec("1"),
            cost_per_contract: dec("1"),
            cost_bps: dec("0"),
            currency_decimals: 2,
        };
        assert_eq!(
            fill_cost(&deep),
            Err(ExecutionError::Overflow { field: "notional" })
        );
    }

    #[test]
    fn price_to_ticks_accepts_grid_points_and_rejects_off_grid_and_overflow() {
        assert_eq!(price_to_ticks(dec("100.25"), dec("0.25")), Ok(401));
        assert_eq!(
            price_to_ticks(dec("100.10"), dec("0.25")),
            Err(ExecutionError::Invalid {
                field: "price",
                reason: "is not a multiple of tick_size",
            })
        );
        assert_eq!(
            price_to_ticks(Decimal::MAX, dec("0.01")),
            Err(ExecutionError::Overflow { field: "price" })
        );
    }
}
