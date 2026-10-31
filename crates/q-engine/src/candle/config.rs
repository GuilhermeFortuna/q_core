use crate::exits::Side;

use super::Sizing;

#[derive(Clone, Copy, Debug)]
pub struct Costs {
    pub per_contract: f64,
    pub bps: f64,
}

#[allow(
    clippy::suboptimal_flops,
    reason = "exact parity with Python: (q * cpc) + ((((bps / 10_000.0) * price) * q) * pv) must not fuse"
)]
pub(crate) fn side_cost(costs: Option<Costs>, price: f64, quantity: f64, point_value: f64) -> f64 {
    let Some(costs) = costs else {
        return 0.0;
    };
    (quantity * costs.per_contract) + ((((costs.bps / 10_000.0) * price) * quantity) * point_value)
}

/// Microseconds since wall-clock midnight.
#[derive(Clone, Copy, Debug)]
pub struct DayTradeWindow {
    pub entry_start_us: i64,
    pub entry_end_us: i64,
    pub force_close_us: i64,
}

#[derive(Clone, Debug)]
pub struct CandleConfig {
    pub initial_capital: f64,
    pub point_value: f64,
    pub costs: Option<Costs>,
    pub sizing: Sizing,
    pub holding_period_bars: Option<i64>,
    pub exit_params: crate::ExitParams,
    pub day_trade: Option<DayTradeWindow>,
    pub force_close_at_end: bool,
    pub latency_us: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CandleError {
    InvalidConfig {
        field: &'static str,
        reason: String,
    },
    LengthMismatch {
        column: String,
        expected: usize,
        actual: usize,
    },
    InvalidSignal {
        column: &'static str,
        bar: usize,
        reason: &'static str,
    },
    Exit(crate::ExitError),
    /// The intrabar source failed, or returned no prices, for a bar that passed the screen.
    IntrabarSource {
        bar: usize,
        reason: String,
    },
    /// A priced entry's price lies outside the range of the bar that decided it.
    EntryPriceOutsideRange {
        bar: usize,
        side: Side,
        price: f64,
        low: f64,
        high: f64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_cost_without_config_is_positive_zero() {
        assert_eq!(side_cost(None, f64::NAN, f64::NAN, f64::NAN).to_bits(), 0);
    }

    #[test]
    fn side_cost_preserves_python_association() {
        let costs = Costs {
            per_contract: 1.5,
            bps: 3.0,
        };
        assert_eq!(
            side_cost(Some(costs), 101.25, 2.0, 0.2).to_bits(),
            0x4008_18e2_1965_2bd4
        );
    }

    #[test]
    fn exact_fill_cost_matches_research_side_cost_within_float_precision() {
        use crate::execution::{fill_cost, FillCost};
        use rust_decimal::Decimal;

        let costs = Costs {
            per_contract: 1.25,
            bps: 3.0,
        };
        // 2 contracts at 500.25 with point value 0.2 and 3 bps, as the research model charges.
        let research = side_cost(Some(costs), 500.25, 2.0, 0.2);
        let exact = fill_cost(&FillCost {
            quantity_steps: 2,
            volume_step: Decimal::ONE,
            price_ticks: 50_025,
            tick_size: Decimal::new(1, 2),
            point_value: Decimal::new(2, 1),
            cost_per_contract: Decimal::new(125, 2),
            cost_bps: Decimal::from(3),
            currency_decimals: 28,
        })
        .unwrap();
        let exact: f64 = exact.to_string().parse().unwrap();
        assert!((research - exact).abs() < 1e-9, "{research} vs {exact}");
    }
}
