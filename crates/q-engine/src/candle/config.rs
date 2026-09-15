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
#[allow(
    dead_code,
    reason = "used by the candle run loop added in the next implementation step"
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
}
