//! Tensor Intent Language — declarative multi-leg trading intents.
//!
//! Provides builder-pattern construction, constraint validation, and
//! helper functions for common strategies (market buy, delta-neutral, etc.).

use tensor_types::*;

/// Constraints on intent execution.
#[derive(Clone, Debug, Default)]
pub struct IntentConstraints {
    /// Max price slippage per leg (BPS)
    pub max_slippage_bps: u16,
    /// Min % of total size that must fill (10000 = all-or-nothing)
    pub min_fill_ratio_bps: u16,
    /// Unix timestamp deadline (0 = no deadline)
    pub deadline: i64,
    /// Max margin consumed by this intent (0 = unlimited)
    pub max_total_cost: u64,
}

/// Target Greeks after execution (for delta-neutral strategies etc).
#[derive(Clone, Debug, Default)]
pub struct TargetGreeks {
    pub target_delta: Option<i64>,
    pub max_abs_gamma: Option<u64>,
    pub max_abs_vega: Option<u64>,
}

/// A multi-leg intent bundle.
#[derive(Clone, Debug)]
pub struct IntentBundle {
    pub legs: Vec<IntentLeg>,
    pub intent_type: IntentType,
    pub constraints: IntentConstraints,
    pub target_greeks: Option<TargetGreeks>,
}

impl IntentBundle {
    pub fn new() -> Self {
        Self {
            legs: Vec::new(),
            intent_type: IntentType::Market,
            constraints: IntentConstraints::default(),
            target_greeks: None,
        }
    }

    pub fn add_leg(mut self, leg: IntentLeg) -> Self {
        self.legs.push(leg);
        self
    }

    pub fn with_type(mut self, t: IntentType) -> Self {
        self.intent_type = t;
        self
    }

    pub fn with_constraints(mut self, c: IntentConstraints) -> Self {
        self.constraints = c;
        self
    }

    pub fn with_target_greeks(mut self, g: TargetGreeks) -> Self {
        self.target_greeks = Some(g);
        self
    }

    pub fn validate(&self) -> Result<(), IntentError> {
        if self.legs.is_empty() {
            return Err(IntentError::EmptyIntent);
        }
        if self.legs.len() > MAX_INTENT_LEGS {
            return Err(IntentError::TooManyLegs);
        }
        for leg in &self.legs {
            if leg.size == 0 {
                return Err(IntentError::ZeroSize);
            }
        }
        if self.constraints.max_slippage_bps > 10000 {
            return Err(IntentError::InvalidConstraint);
        }
        if self.constraints.min_fill_ratio_bps > 10000 {
            return Err(IntentError::InvalidConstraint);
        }
        Ok(())
    }

    pub fn leg_count(&self) -> usize {
        self.legs.len()
    }

    /// Estimate total notional across all legs: sum of |size * price|.
    pub fn total_notional_estimate(&self, prices: &[u64]) -> u64 {
        let precision = 1_000_000u128;
        self.legs
            .iter()
            .map(|leg| {
                let price = prices
                    .get(leg.market_index as usize)
                    .copied()
                    .unwrap_or(0);
                let abs_size = leg.size.unsigned_abs() as u128;
                (abs_size * price as u128 / precision) as u64
            })
            .sum()
    }
}

impl Default for IntentBundle {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Builder helpers for common strategies
// ---------------------------------------------------------------------------

/// Single-leg market buy perp.
pub fn market_buy_perp(market_index: u16, size: i64) -> IntentBundle {
    IntentBundle::new()
        .with_type(IntentType::Market)
        .add_leg(IntentLeg {
            product_type: ProductType::Perpetual,
            market_index,
            size: size.abs(),
            limit_price: 0,
            is_active: true,
        })
}

/// Single-leg limit sell perp.
pub fn limit_sell_perp(market_index: u16, size: i64, price: u64) -> IntentBundle {
    IntentBundle::new()
        .with_type(IntentType::Limit)
        .add_leg(IntentLeg {
            product_type: ProductType::Perpetual,
            market_index,
            size: -(size.abs()),
            limit_price: price,
            is_active: true,
        })
}

/// Delta-neutral: long spot + short perp of equal size.
pub fn delta_neutral_perp_spot(market_index: u16, size: i64) -> IntentBundle {
    let abs_size = size.abs();
    IntentBundle::new()
        .with_type(IntentType::Market)
        .add_leg(IntentLeg {
            product_type: ProductType::Spot,
            market_index,
            size: abs_size,
            limit_price: 0,
            is_active: true,
        })
        .add_leg(IntentLeg {
            product_type: ProductType::Perpetual,
            market_index,
            size: -abs_size,
            limit_price: 0,
            is_active: true,
        })
}

/// Covered call: long spot + short call option.
pub fn covered_call(market_index: u16, spot_size: i64, strike: u64) -> IntentBundle {
    let abs_size = spot_size.abs();
    IntentBundle::new()
        .with_type(IntentType::Limit)
        .add_leg(IntentLeg {
            product_type: ProductType::Spot,
            market_index,
            size: abs_size,
            limit_price: 0,
            is_active: true,
        })
        .add_leg(IntentLeg {
            product_type: ProductType::Option,
            market_index,
            size: -abs_size,
            limit_price: strike,
            is_active: true,
        })
}

#[derive(Debug, PartialEq)]
pub enum IntentError {
    TooManyLegs,
    EmptyIntent,
    ZeroSize,
    InvalidConstraint,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intent_bundle_new_empty() {
        let bundle = IntentBundle::new();
        assert_eq!(bundle.leg_count(), 0);
        assert_eq!(bundle.intent_type, IntentType::Market);
        assert!(bundle.target_greeks.is_none());
    }

    #[test]
    fn test_add_leg_single() {
        let bundle = IntentBundle::new().add_leg(IntentLeg {
            product_type: ProductType::Perpetual,
            market_index: 0,
            size: 10_000_000,
            limit_price: 0,
            is_active: true,
        });
        assert_eq!(bundle.leg_count(), 1);
    }

    #[test]
    fn test_add_leg_multiple() {
        let bundle = IntentBundle::new()
            .add_leg(IntentLeg {
                product_type: ProductType::Perpetual,
                market_index: 0,
                size: 10_000_000,
                limit_price: 0,
                is_active: true,
            })
            .add_leg(IntentLeg {
                product_type: ProductType::Spot,
                market_index: 1,
                size: 5_000_000,
                limit_price: 0,
                is_active: true,
            });
        assert_eq!(bundle.leg_count(), 2);
    }

    #[test]
    fn test_add_leg_max() {
        let mut bundle = IntentBundle::new();
        for i in 0..MAX_INTENT_LEGS {
            bundle = bundle.add_leg(IntentLeg {
                product_type: ProductType::Perpetual,
                market_index: i as u16,
                size: 1_000_000,
                limit_price: 0,
                is_active: true,
            });
        }
        assert_eq!(bundle.leg_count(), MAX_INTENT_LEGS);
        assert!(bundle.validate().is_ok());
    }

    #[test]
    fn test_add_leg_overflow() {
        let mut bundle = IntentBundle::new();
        for i in 0..=MAX_INTENT_LEGS {
            bundle = bundle.add_leg(IntentLeg {
                product_type: ProductType::Perpetual,
                market_index: i as u16,
                size: 1_000_000,
                limit_price: 0,
                is_active: true,
            });
        }
        assert_eq!(bundle.validate(), Err(IntentError::TooManyLegs));
    }

    #[test]
    fn test_validate_empty() {
        let bundle = IntentBundle::new();
        assert_eq!(bundle.validate(), Err(IntentError::EmptyIntent));
    }

    #[test]
    fn test_validate_valid() {
        let bundle = IntentBundle::new().add_leg(IntentLeg {
            product_type: ProductType::Perpetual,
            market_index: 0,
            size: 10_000_000,
            limit_price: 0,
            is_active: true,
        });
        assert!(bundle.validate().is_ok());
    }

    #[test]
    fn test_validate_zero_size() {
        let bundle = IntentBundle::new().add_leg(IntentLeg {
            product_type: ProductType::Perpetual,
            market_index: 0,
            size: 0,
            limit_price: 0,
            is_active: true,
        });
        assert_eq!(bundle.validate(), Err(IntentError::ZeroSize));
    }

    #[test]
    fn test_validate_invalid_slippage() {
        let bundle = IntentBundle::new()
            .add_leg(IntentLeg {
                product_type: ProductType::Perpetual,
                market_index: 0,
                size: 1_000_000,
                limit_price: 0,
                is_active: true,
            })
            .with_constraints(IntentConstraints {
                max_slippage_bps: 10001,
                ..Default::default()
            });
        assert_eq!(bundle.validate(), Err(IntentError::InvalidConstraint));
    }

    #[test]
    fn test_validate_invalid_fill_ratio() {
        let bundle = IntentBundle::new()
            .add_leg(IntentLeg {
                product_type: ProductType::Perpetual,
                market_index: 0,
                size: 1_000_000,
                limit_price: 0,
                is_active: true,
            })
            .with_constraints(IntentConstraints {
                min_fill_ratio_bps: 10001,
                ..Default::default()
            });
        assert_eq!(bundle.validate(), Err(IntentError::InvalidConstraint));
    }

    #[test]
    fn test_total_notional_estimate() {
        let bundle = IntentBundle::new()
            .add_leg(IntentLeg {
                product_type: ProductType::Perpetual,
                market_index: 0,
                size: 10_000_000, // 10 units
                limit_price: 0,
                is_active: true,
            })
            .add_leg(IntentLeg {
                product_type: ProductType::Spot,
                market_index: 1,
                size: -5_000_000, // 5 units (short)
                limit_price: 0,
                is_active: true,
            });
        let prices = vec![150_000_000u64, 60_000_000_000u64]; // SOL $150, BTC $60000
        // |10 * 150| + |5 * 60000| = 1500 + 300000 = 301500
        let notional = bundle.total_notional_estimate(&prices);
        assert_eq!(notional, 1_500_000_000 + 300_000_000_000);
    }

    #[test]
    fn test_market_buy_perp() {
        let bundle = market_buy_perp(0, 10_000_000);
        assert_eq!(bundle.leg_count(), 1);
        assert_eq!(bundle.intent_type, IntentType::Market);
        assert_eq!(bundle.legs[0].size, 10_000_000);
        assert_eq!(bundle.legs[0].product_type, ProductType::Perpetual);
        assert!(bundle.validate().is_ok());
    }

    #[test]
    fn test_limit_sell_perp() {
        let bundle = limit_sell_perp(0, 10_000_000, 150_000_000);
        assert_eq!(bundle.leg_count(), 1);
        assert_eq!(bundle.intent_type, IntentType::Limit);
        assert_eq!(bundle.legs[0].size, -10_000_000);
        assert_eq!(bundle.legs[0].limit_price, 150_000_000);
        assert!(bundle.validate().is_ok());
    }

    #[test]
    fn test_delta_neutral_perp_spot() {
        let bundle = delta_neutral_perp_spot(0, 100_000_000);
        assert_eq!(bundle.leg_count(), 2);
        assert_eq!(bundle.legs[0].product_type, ProductType::Spot);
        assert_eq!(bundle.legs[0].size, 100_000_000);
        assert_eq!(bundle.legs[1].product_type, ProductType::Perpetual);
        assert_eq!(bundle.legs[1].size, -100_000_000);
        assert!(bundle.validate().is_ok());
    }

    #[test]
    fn test_covered_call() {
        let bundle = covered_call(0, 10_000_000, 160_000_000);
        assert_eq!(bundle.leg_count(), 2);
        assert_eq!(bundle.legs[0].product_type, ProductType::Spot);
        assert_eq!(bundle.legs[0].size, 10_000_000);
        assert_eq!(bundle.legs[1].product_type, ProductType::Option);
        assert_eq!(bundle.legs[1].size, -10_000_000);
        assert_eq!(bundle.legs[1].limit_price, 160_000_000);
        assert!(bundle.validate().is_ok());
    }

    #[test]
    fn test_intent_constraints_defaults() {
        let c = IntentConstraints::default();
        assert_eq!(c.max_slippage_bps, 0);
        assert_eq!(c.min_fill_ratio_bps, 0);
        assert_eq!(c.deadline, 0);
        assert_eq!(c.max_total_cost, 0);
    }

    #[test]
    fn test_target_greeks_optional_fields() {
        let g = TargetGreeks::default();
        assert!(g.target_delta.is_none());
        assert!(g.max_abs_gamma.is_none());
        assert!(g.max_abs_vega.is_none());

        let g2 = TargetGreeks {
            target_delta: Some(0),
            max_abs_gamma: Some(100_000),
            max_abs_vega: None,
        };
        assert_eq!(g2.target_delta, Some(0));
        assert_eq!(g2.max_abs_gamma, Some(100_000));
        assert!(g2.max_abs_vega.is_none());
    }

    #[test]
    fn test_with_type() {
        let bundle = IntentBundle::new().with_type(IntentType::Conditional);
        assert_eq!(bundle.intent_type, IntentType::Conditional);
    }

    #[test]
    fn test_with_target_greeks() {
        let bundle = IntentBundle::new().with_target_greeks(TargetGreeks {
            target_delta: Some(0),
            ..Default::default()
        });
        assert!(bundle.target_greeks.is_some());
        assert_eq!(bundle.target_greeks.unwrap().target_delta, Some(0));
    }
}
