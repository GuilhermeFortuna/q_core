use super::CandleError;

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

impl Sizing {
    pub fn validate(&self) -> Result<(), CandleError> {
        match self {
            Self::FixedQuantity { .. } => Ok(()),
            Self::FixedSafetyMargin {
                margin_per_contract,
                min_contracts,
                max_contracts,
                ..
            } => {
                if *margin_per_contract <= 0.0 {
                    return Err(invalid_config(
                        "margin_per_contract",
                        "must be greater than zero",
                    ));
                }
                if *min_contracts < 0 {
                    return Err(invalid_config("min_contracts", "must be non-negative"));
                }
                if max_contracts.is_some_and(|maximum| maximum < *min_contracts) {
                    return Err(invalid_config(
                        "max_contracts",
                        "must be greater than or equal to min_contracts",
                    ));
                }
                Ok(())
            }
            Self::InverseVolatility {
                target_volatility_pct,
                point_value,
                min_contracts,
                max_contracts,
                ..
            } => {
                if *target_volatility_pct <= 0.0 {
                    return Err(invalid_config(
                        "target_volatility_pct",
                        "must be greater than zero",
                    ));
                }
                if *point_value <= 0.0 {
                    return Err(invalid_config("point_value", "must be greater than zero"));
                }
                if *min_contracts < 0 {
                    return Err(invalid_config("min_contracts", "must be non-negative"));
                }
                if max_contracts.is_some_and(|maximum| maximum < *min_contracts) {
                    return Err(invalid_config(
                        "max_contracts",
                        "must be greater than or equal to min_contracts",
                    ));
                }
                Ok(())
            }
        }
    }

    pub fn size(
        &self,
        strength: f64,
        price: f64,
        capital: f64,
        volatility: Option<f64>,
    ) -> Option<f64> {
        let quantity = match self {
            Self::FixedQuantity {
                quantity,
                scale_by_strength,
            } => {
                let quantity = if *scale_by_strength {
                    quantity * strength
                } else {
                    *quantity
                };
                quantity.floor()
            }
            Self::FixedSafetyMargin {
                scale_by_strength, ..
            } => {
                let quantity = self.safety_margin_target(capital);
                if *scale_by_strength {
                    (quantity * strength).floor()
                } else {
                    quantity
                }
            }
            Self::InverseVolatility {
                scale_by_strength, ..
            } => {
                let quantity = self.inverse_volatility_target(price, capital, volatility)?;
                if *scale_by_strength {
                    (quantity * strength).floor()
                } else {
                    quantity
                }
            }
        };
        (quantity > 0.0).then_some(quantity)
    }

    pub fn max_position(&self, _price: f64, capital: f64) -> Option<f64> {
        match self {
            Self::FixedQuantity { quantity, .. } => Some(*quantity),
            Self::FixedSafetyMargin { .. } => Some(self.safety_margin_target(capital)),
            Self::InverseVolatility { max_contracts, .. } => {
                max_contracts.map(|maximum| maximum as f64)
            }
        }
    }

    #[allow(
        clippy::float_cmp,
        reason = "the floored contract count is integral and Python distinguishes exactly zero"
    )]
    fn safety_margin_target(&self, capital: f64) -> f64 {
        let Self::FixedSafetyMargin {
            margin_per_contract,
            min_contracts,
            max_contracts,
            ..
        } = self
        else {
            unreachable!("called only for FixedSafetyMargin")
        };

        let mut quantity = (capital / margin_per_contract).floor();
        if let Some(maximum) = max_contracts {
            if quantity > *maximum as f64 {
                quantity = *maximum as f64;
            }
        }
        if quantity < *min_contracts as f64 {
            if *min_contracts > 0 && quantity == 0.0 {
                quantity = *min_contracts as f64;
            } else {
                return 0.0;
            }
        }
        if quantity > 0.0 {
            quantity
        } else {
            0.0
        }
    }

    #[allow(
        clippy::suboptimal_flops,
        reason = "exact parity with Python: ((target / 100.0) * capital) / ((vol * price) * point_value) must not fuse"
    )]
    fn inverse_volatility_target(
        &self,
        price: f64,
        capital: f64,
        volatility: Option<f64>,
    ) -> Option<f64> {
        let Self::InverseVolatility {
            target_volatility_pct,
            point_value,
            min_contracts,
            max_contracts,
            ..
        } = self
        else {
            unreachable!("called only for InverseVolatility")
        };

        let volatility = volatility?;
        if volatility.is_nan() || volatility <= 0.0 || price <= 0.0 || capital <= 0.0 {
            return None;
        }
        let notional = (volatility * price) * point_value;
        if notional <= 0.0 {
            return None;
        }
        let mut quantity = (((target_volatility_pct / 100.0) * capital) / notional).floor();
        if let Some(maximum) = max_contracts {
            if quantity > *maximum as f64 {
                quantity = *maximum as f64;
            }
        }
        if quantity < *min_contracts as f64 {
            quantity = *min_contracts as f64;
        }
        (quantity > 0.0).then_some(quantity)
    }
}

fn invalid_config(field: &'static str, reason: &str) -> CandleError {
    CandleError::InvalidConfig {
        field,
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invalid_field(result: Result<(), CandleError>) -> &'static str {
        match result {
            Err(CandleError::InvalidConfig { field, .. }) => field,
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn fixed_quantity_floors_scaled_quantity_and_reports_unfloored_maximum() {
        let sizing = Sizing::FixedQuantity {
            quantity: 1.9,
            scale_by_strength: false,
        };
        assert_eq!(sizing.size(1.0, 100.0, 10_000.0, None), Some(1.0));
        assert_eq!(sizing.max_position(100.0, 10_000.0), Some(1.9));

        let scaled = Sizing::FixedQuantity {
            quantity: 2.0,
            scale_by_strength: true,
        };
        assert_eq!(scaled.size(0.4, 100.0, 10_000.0, None), None);
        assert!(scaled.validate().is_ok());
    }

    #[test]
    fn safety_margin_matches_python_minimum_and_maximum_rules() {
        let sizing = Sizing::FixedSafetyMargin {
            margin_per_contract: 5_000.0,
            min_contracts: 0,
            max_contracts: None,
            scale_by_strength: false,
        };
        assert_eq!(sizing.size(1.0, 100.0, 12_000.0, None), Some(2.0));
        assert_eq!(sizing.max_position(100.0, 12_000.0), Some(2.0));

        let capped = Sizing::FixedSafetyMargin {
            margin_per_contract: 5_000.0,
            min_contracts: 0,
            max_contracts: Some(1),
            scale_by_strength: false,
        };
        assert_eq!(capped.size(1.0, 100.0, 12_000.0, None), Some(1.0));

        let minimum = Sizing::FixedSafetyMargin {
            margin_per_contract: 5_000.0,
            min_contracts: 2,
            max_contracts: None,
            scale_by_strength: false,
        };
        assert_eq!(minimum.size(1.0, 100.0, 3_000.0, None), Some(2.0));
        assert_eq!(minimum.size(1.0, 100.0, 7_000.0, None), None);

        let positive_minimum = Sizing::FixedSafetyMargin {
            margin_per_contract: 5_000.0,
            min_contracts: 1,
            max_contracts: None,
            scale_by_strength: false,
        };
        assert_eq!(positive_minimum.size(1.0, 100.0, -1_000.0, None), None);
    }

    #[test]
    fn inverse_volatility_rejects_invalid_volatility_and_uses_maximum_fallback() {
        let sizing = Sizing::InverseVolatility {
            target_volatility_pct: 10.0,
            point_value: 0.2,
            min_contracts: 0,
            max_contracts: Some(5),
            scale_by_strength: false,
        };
        assert_eq!(sizing.size(1.0, 100.0, 100_000.0, Some(0.25)), Some(5.0));
        for volatility in [Some(f64::NAN), Some(0.0), Some(-0.1), None] {
            assert_eq!(sizing.size(1.0, 100.0, 100_000.0, volatility), None);
        }
        assert_eq!(sizing.max_position(100.0, 100_000.0), Some(5.0));

        let uncapped = Sizing::InverseVolatility {
            target_volatility_pct: 10.0,
            point_value: 0.2,
            min_contracts: 0,
            max_contracts: None,
            scale_by_strength: false,
        };
        assert_eq!(uncapped.max_position(100.0, 100_000.0), None);
    }

    #[test]
    fn validation_uses_python_check_order_and_field_names() {
        let invalid_margin = Sizing::FixedSafetyMargin {
            margin_per_contract: 0.0,
            min_contracts: -1,
            max_contracts: Some(-2),
            scale_by_strength: false,
        };
        assert_eq!(
            invalid_field(invalid_margin.validate()),
            "margin_per_contract"
        );

        let invalid_safety_min = Sizing::FixedSafetyMargin {
            margin_per_contract: 1.0,
            min_contracts: -1,
            max_contracts: Some(-2),
            scale_by_strength: false,
        };
        assert_eq!(
            invalid_field(invalid_safety_min.validate()),
            "min_contracts"
        );

        let invalid_safety_max = Sizing::FixedSafetyMargin {
            margin_per_contract: 1.0,
            min_contracts: 2,
            max_contracts: Some(1),
            scale_by_strength: false,
        };
        assert_eq!(
            invalid_field(invalid_safety_max.validate()),
            "max_contracts"
        );

        let invalid_target = Sizing::InverseVolatility {
            target_volatility_pct: 0.0,
            point_value: 0.0,
            min_contracts: -1,
            max_contracts: Some(-2),
            scale_by_strength: false,
        };
        assert_eq!(
            invalid_field(invalid_target.validate()),
            "target_volatility_pct"
        );

        let invalid_point_value = Sizing::InverseVolatility {
            target_volatility_pct: 1.0,
            point_value: 0.0,
            min_contracts: -1,
            max_contracts: Some(-2),
            scale_by_strength: false,
        };
        assert_eq!(invalid_field(invalid_point_value.validate()), "point_value");

        let invalid_inverse_min = Sizing::InverseVolatility {
            target_volatility_pct: 1.0,
            point_value: 1.0,
            min_contracts: -1,
            max_contracts: Some(-2),
            scale_by_strength: false,
        };
        assert_eq!(
            invalid_field(invalid_inverse_min.validate()),
            "min_contracts"
        );

        let invalid_inverse_max = Sizing::InverseVolatility {
            target_volatility_pct: 1.0,
            point_value: 1.0,
            min_contracts: 2,
            max_contracts: Some(1),
            scale_by_strength: false,
        };
        assert_eq!(
            invalid_field(invalid_inverse_max.validate()),
            "max_contracts"
        );
    }
}
