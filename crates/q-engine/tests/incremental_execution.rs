//! Vectors for the stateless execution primitives in `q_engine::execution`.
//!
//! The Python binding tests (`tests/test_incremental_execution.py`) use the same vectors.

use q_engine::candle::RestingKind;
use q_engine::execution::{
    fill_cost, match_quote, match_trade, price_to_ticks, ExecutionError, FillCost, OrderKind,
    OrderSide, Quote,
};
use rust_decimal::Decimal;

fn quote(bid: i64, ask: i64) -> Quote {
    Quote { bid, ask }
}

fn dec(text: &str) -> Decimal {
    Decimal::from_str_exact(text).unwrap()
}

fn quote_fill(
    side: OrderSide,
    kind: OrderKind,
    level: Option<i64>,
    q: Quote,
    slip: i64,
) -> Option<i64> {
    match_quote(side, kind, level, q, slip).unwrap()
}

#[test]
fn quote_model_touch_fills_a_buy_limit_at_its_level_and_trade_model_does_not() {
    // The explicit distinction: ask = 100 and buy limit = 100 fill under the quote policy.
    assert_eq!(
        quote_fill(
            OrderSide::Buy,
            OrderKind::Limit,
            Some(100),
            quote(99, 100),
            0
        ),
        Some(100)
    );
    // A trade print of 100 does not fill the same limit under the trade policy.
    assert_eq!(
        match_trade(OrderSide::Buy, RestingKind::Limit, 100, 100).unwrap(),
        None
    );
    assert_eq!(
        match_trade(OrderSide::Buy, RestingKind::Limit, 100, 99).unwrap(),
        Some(100)
    );
}

#[test]
fn quote_model_stop_touch_triggers_at_the_level_on_both_sides() {
    assert_eq!(
        quote_fill(
            OrderSide::Buy,
            OrderKind::Stop,
            Some(100),
            quote(99, 100),
            0
        ),
        Some(100)
    );
    assert_eq!(
        quote_fill(OrderSide::Buy, OrderKind::Stop, Some(100), quote(98, 99), 0),
        None
    );
    assert_eq!(
        quote_fill(
            OrderSide::Sell,
            OrderKind::Stop,
            Some(100),
            quote(100, 101),
            0
        ),
        Some(100)
    );
    assert_eq!(
        quote_fill(
            OrderSide::Sell,
            OrderKind::Stop,
            Some(100),
            quote(101, 102),
            0
        ),
        None
    );
}

#[test]
fn quote_model_stop_gap_fills_at_the_executable_quote_not_the_level() {
    // A buy stop at 100 gapped to ask 105 fills as a market buy at 105.
    assert_eq!(
        quote_fill(
            OrderSide::Buy,
            OrderKind::Stop,
            Some(100),
            quote(104, 105),
            0
        ),
        Some(105)
    );
    // A sell stop at 100 gapped to bid 95 fills as a market sell at 95.
    assert_eq!(
        quote_fill(
            OrderSide::Sell,
            OrderKind::Stop,
            Some(100),
            quote(95, 96),
            0
        ),
        Some(95)
    );
}

#[test]
fn quote_model_limit_improvement_fills_at_the_better_quote() {
    // A buy limit at 100 with ask 98 fills at 98: two ticks of improvement.
    assert_eq!(
        quote_fill(
            OrderSide::Buy,
            OrderKind::Limit,
            Some(100),
            quote(97, 98),
            0
        ),
        Some(98)
    );
    // A sell limit at 100 with bid 103 fills at 103.
    assert_eq!(
        quote_fill(
            OrderSide::Sell,
            OrderKind::Limit,
            Some(100),
            quote(103, 104),
            0
        ),
        Some(103)
    );
    // A buy limit does not fill while the ask stays above it.
    assert_eq!(
        quote_fill(
            OrderSide::Buy,
            OrderKind::Limit,
            Some(100),
            quote(100, 101),
            0
        ),
        None
    );
}

#[test]
fn quote_model_market_orders_pay_the_quote_plus_adverse_slippage() {
    assert_eq!(
        quote_fill(OrderSide::Buy, OrderKind::Market, None, quote(99, 100), 2),
        Some(102)
    );
    assert_eq!(
        quote_fill(OrderSide::Sell, OrderKind::Market, None, quote(99, 100), 2),
        Some(97)
    );
}

#[test]
fn quote_model_slippage_applies_to_stops_but_never_to_limits() {
    // Stop buy triggered at ask 100 fills at 100 + 3 slippage.
    assert_eq!(
        quote_fill(
            OrderSide::Buy,
            OrderKind::Stop,
            Some(100),
            quote(99, 100),
            3
        ),
        Some(103)
    );
    // Limit buy at 100 with ask 98 fills at 98 regardless of slippage.
    assert_eq!(
        quote_fill(
            OrderSide::Buy,
            OrderKind::Limit,
            Some(100),
            quote(97, 98),
            3
        ),
        Some(98)
    );
    // Limit sell at 100 with bid 103 fills at 103 regardless of slippage.
    assert_eq!(
        quote_fill(
            OrderSide::Sell,
            OrderKind::Limit,
            Some(100),
            quote(103, 104),
            3
        ),
        Some(103)
    );
}

#[test]
fn quote_model_rejects_invalid_quotes_levels_and_slippage() {
    let invalid = |side, kind, level, q, slip| {
        match_quote(side, kind, level, q, slip).expect_err("expected an invalid input")
    };
    assert!(matches!(
        invalid(OrderSide::Buy, OrderKind::Market, None, quote(101, 100), 0),
        ExecutionError::Invalid { field: "quote", .. }
    ));
    assert!(matches!(
        invalid(OrderSide::Buy, OrderKind::Market, None, quote(0, 100), 0),
        ExecutionError::Invalid {
            field: "bid_ticks",
            ..
        }
    ));
    assert!(matches!(
        invalid(OrderSide::Buy, OrderKind::Market, None, quote(99, -1), 0),
        ExecutionError::Invalid {
            field: "ask_ticks",
            ..
        }
    ));
    assert!(matches!(
        invalid(OrderSide::Buy, OrderKind::Market, None, quote(99, 100), -1),
        ExecutionError::Invalid {
            field: "slippage_ticks",
            ..
        }
    ));
    assert!(matches!(
        invalid(
            OrderSide::Buy,
            OrderKind::Market,
            Some(100),
            quote(99, 100),
            0
        ),
        ExecutionError::Invalid {
            field: "level_ticks",
            ..
        }
    ));
    assert!(matches!(
        invalid(OrderSide::Buy, OrderKind::Stop, None, quote(99, 100), 0),
        ExecutionError::Invalid {
            field: "level_ticks",
            ..
        }
    ));
    assert!(matches!(
        invalid(OrderSide::Buy, OrderKind::Limit, Some(0), quote(99, 100), 0),
        ExecutionError::Invalid {
            field: "level_ticks",
            ..
        }
    ));
}

#[test]
fn sell_market_fill_must_stay_positive_after_slippage() {
    assert!(matches!(
        match_quote(OrderSide::Sell, OrderKind::Market, None, quote(3, 4), 3),
        Err(ExecutionError::Invalid {
            field: "fill_ticks",
            ..
        })
    ));
}

#[test]
fn buy_fill_overflow_is_reported_instead_of_wrapping() {
    let big = i64::MAX - 1;
    assert_eq!(
        match_quote(OrderSide::Buy, OrderKind::Market, None, quote(big, big), 1).unwrap(),
        Some(i64::MAX)
    );
    assert_eq!(
        match_quote(OrderSide::Buy, OrderKind::Market, None, quote(big, big), 2),
        Err(ExecutionError::Overflow {
            field: "fill_ticks"
        })
    );
}

#[test]
fn large_valid_prices_and_minimum_size_match_without_loss() {
    let big = i64::MAX / 4;
    assert_eq!(
        quote_fill(
            OrderSide::Sell,
            OrderKind::Stop,
            Some(big),
            quote(big, big + 1),
            0
        ),
        Some(big)
    );
    assert_eq!(
        match_trade(OrderSide::Sell, RestingKind::Stop, big, big).unwrap(),
        Some(big)
    );
    // Minimum size: the smallest positive tick and the smallest positive level.
    assert_eq!(
        quote_fill(OrderSide::Buy, OrderKind::Market, None, quote(1, 1), 0),
        Some(1)
    );
    assert_eq!(
        match_trade(OrderSide::Buy, RestingKind::Stop, 1, 1).unwrap(),
        Some(1)
    );
}

#[test]
fn repeated_calls_with_the_same_arguments_are_identical() {
    let first = quote_fill(
        OrderSide::Buy,
        OrderKind::Stop,
        Some(100),
        quote(99, 105),
        2,
    );
    for _ in 0..1000 {
        assert_eq!(
            quote_fill(
                OrderSide::Buy,
                OrderKind::Stop,
                Some(100),
                quote(99, 105),
                2
            ),
            first
        );
    }
}

#[test]
fn trade_model_rejects_nonpositive_levels_and_trades() {
    assert!(matches!(
        match_trade(OrderSide::Buy, RestingKind::Limit, 0, 100),
        Err(ExecutionError::Invalid {
            field: "level_ticks",
            ..
        })
    ));
    assert!(matches!(
        match_trade(OrderSide::Buy, RestingKind::Stop, 100, -5),
        Err(ExecutionError::Invalid {
            field: "trade_ticks",
            ..
        })
    ));
}

fn fee_inputs() -> FillCost {
    FillCost {
        quantity_steps: 3,
        volume_step: dec("0.01"),
        price_ticks: 1000,
        tick_size: dec("0.5"),
        point_value: dec("2"),
        cost_per_contract: dec("1.25"),
        cost_bps: dec("1.5"),
        currency_decimals: 2,
    }
}

#[test]
fn fill_cost_applies_per_contract_and_basis_point_fees() {
    // quantity 0.03 @ 500 with point value 2: notional 30. Per contract 0.0375; bps 30 * 1.5 / 10000 = 0.0045.
    // Total 0.042, rounded to 0.04.
    assert_eq!(fill_cost(&fee_inputs()).unwrap().to_string(), "0.04");
    // Four decimal places show the exact pre-rounding total of 0.0420.
    let precise = FillCost {
        currency_decimals: 4,
        ..fee_inputs()
    };
    assert_eq!(fill_cost(&precise).unwrap().to_string(), "0.0420");
}

#[test]
fn fill_cost_zero_quantity_and_zero_fee_rates_give_zero() {
    let free = FillCost {
        quantity_steps: 0,
        cost_per_contract: dec("0"),
        cost_bps: dec("0"),
        ..fee_inputs()
    };
    assert_eq!(fill_cost(&free).unwrap().to_string(), "0.00");
}

#[test]
fn fill_cost_rounds_half_to_even_at_currency_precision() {
    let at = |per_contract: &str| FillCost {
        quantity_steps: 1,
        volume_step: dec("1"),
        price_ticks: 1,
        tick_size: dec("1"),
        point_value: dec("1"),
        cost_per_contract: dec(per_contract),
        cost_bps: dec("0"),
        currency_decimals: 2,
    };
    assert_eq!(fill_cost(&at("0.125")).unwrap().to_string(), "0.12");
    assert_eq!(fill_cost(&at("0.135")).unwrap().to_string(), "0.14");
    assert_eq!(fill_cost(&at("0.145")).unwrap().to_string(), "0.14");
}

#[test]
fn fill_cost_rejects_negative_sizes_costs_and_rates() {
    let base = fee_inputs();
    assert!(matches!(
        fill_cost(&FillCost {
            quantity_steps: -1,
            ..base.clone()
        }),
        Err(ExecutionError::Invalid {
            field: "quantity_steps",
            ..
        })
    ));
    assert!(matches!(
        fill_cost(&FillCost {
            cost_per_contract: dec("-0.01"),
            ..base.clone()
        }),
        Err(ExecutionError::Invalid {
            field: "cost_per_contract",
            ..
        })
    ));
    assert!(matches!(
        fill_cost(&FillCost {
            price_ticks: 0,
            ..base
        }),
        Err(ExecutionError::Invalid {
            field: "price_ticks",
            ..
        })
    ));
}

#[test]
fn fill_cost_reports_overflow_rather_than_rounding_a_product() {
    // Quantity scale 28 times price scale 28 is beyond Decimal's exact range.
    let deep = FillCost {
        volume_step: dec("0.0000000000000000000000000001"),
        tick_size: dec("0.0000000000000000000000000001"),
        ..fee_inputs()
    };
    assert!(matches!(
        fill_cost(&deep),
        Err(ExecutionError::Overflow { .. })
    ));
}

#[test]
fn price_to_ticks_converts_on_grid_prices_and_rejects_off_grid_levels() {
    assert_eq!(price_to_ticks(dec("100.25"), dec("0.25")), Ok(401));
    assert_eq!(price_to_ticks(dec("5"), dec("0.5")), Ok(10));
    assert!(matches!(
        price_to_ticks(dec("100.10"), dec("0.25")),
        Err(ExecutionError::Invalid { field: "price", .. })
    ));
    assert!(matches!(
        price_to_ticks(dec("0"), dec("0.25")),
        Err(ExecutionError::Invalid { field: "price", .. })
    ));
}
