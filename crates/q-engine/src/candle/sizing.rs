/// Position-sizing model used for candle entries.
#[derive(Clone, Debug)]
pub enum Sizing {
    FixedQuantity {
        quantity: f64,
        scale_by_strength: bool,
    },
    FixedSafetyMargin {
        margin_per_contract: f64,
        min_contracts: i64,
        max_contracts: Option<i64>,
        scale_by_strength: bool,
    },
    InverseVolatility {
        target_volatility_pct: f64,
        point_value: f64,
        min_contracts: i64,
        max_contracts: Option<i64>,
        scale_by_strength: bool,
    },
}
