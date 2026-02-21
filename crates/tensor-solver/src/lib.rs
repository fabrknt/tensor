//! Tensor Solver — off-chain intent decomposition and margin simulation.
//!
//! Decomposes multi-leg intent bundles into ordered execution steps,
//! optimizes execution order to minimize peak margin usage, and
//! simulates margin impact to determine feasibility.

use tensor_types::*;
use tensor_intents::*;

/// Execution step produced by the solver.
#[derive(Clone, Debug)]
pub struct ExecutionStep {
    pub leg_index: usize,
    pub product_type: ProductType,
    pub market_index: u16,
    pub size: i64,
    pub estimated_price: u64,
    pub estimated_margin_delta: i64,
}

/// Solver result.
#[derive(Clone, Debug)]
pub struct SolverResult {
    pub steps: Vec<ExecutionStep>,
    pub estimated_total_margin: u64,
    pub estimated_total_cost: u64,
    pub feasible: bool,
    pub reason: Option<String>,
}

/// Configuration for margin simulation.
#[derive(Clone, Debug)]
pub struct MarginSimConfig {
    pub initial_margin_bps: u64,
    pub maintenance_ratio_bps: u64,
    pub gamma_margin_bps: u64,
    pub vega_margin_bps: u64,
    pub credit_discount_bps: u64,
}

impl Default for MarginSimConfig {
    fn default() -> Self {
        Self {
            initial_margin_bps: 1000,
            maintenance_ratio_bps: 5000,
            gamma_margin_bps: 100,
            vega_margin_bps: 50,
            credit_discount_bps: 0,
        }
    }
}

/// Decompose an intent bundle into execution steps.
/// Each leg becomes one step with estimated price from its limit_price or a provided price.
pub fn decompose_intent(bundle: &IntentBundle, prices: &[u64]) -> Vec<ExecutionStep> {
    bundle
        .legs
        .iter()
        .enumerate()
        .map(|(i, leg)| {
            let price = if leg.limit_price > 0 {
                leg.limit_price
            } else {
                prices.get(leg.market_index as usize).copied().unwrap_or(0)
            };

            let precision = 1_000_000u128;
            let abs_size = leg.size.unsigned_abs() as u128;
            let notional = (abs_size * price as u128 / precision) as i64;

            ExecutionStep {
                leg_index: i,
                product_type: leg.product_type,
                market_index: leg.market_index,
                size: leg.size,
                estimated_price: price,
                estimated_margin_delta: notional,
            }
        })
        .collect()
}

/// Optimize execution order to minimize peak margin usage.
///
/// Strategy: execute hedging legs first to benefit from delta-netting.
/// A hedging leg is one that reduces net delta (i.e., its sign opposes the
/// cumulative delta of the portfolio).
pub fn optimize_execution_order(steps: &mut Vec<ExecutionStep>, current_greeks: &PortfolioGreeks) {
    let current_delta = current_greeks.delta;

    // Sort: legs that reduce delta magnitude come first
    steps.sort_by(|a, b| {
        let a_reduces = (current_delta > 0 && a.size < 0) || (current_delta < 0 && a.size > 0);
        let b_reduces = (current_delta > 0 && b.size < 0) || (current_delta < 0 && b.size > 0);

        // Delta-reducing legs first
        match (a_reduces, b_reduces) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.leg_index.cmp(&b.leg_index), // preserve original order otherwise
        }
    });
}

/// Simulate margin impact of executing steps in order.
///
/// Tracks cumulative delta to compute margin requirements at each step,
/// applying delta-netting. Returns a SolverResult indicating whether the
/// trade is feasible given the available collateral.
pub fn simulate_margin_impact(
    steps: &[ExecutionStep],
    current_greeks: &PortfolioGreeks,
    collateral: u64,
    config: &MarginSimConfig,
) -> SolverResult {
    let precision = 1_000_000u128;
    let bps = 10_000u128;

    let mut cumulative_delta = current_greeks.delta as i128;
    let mut peak_margin: u64 = 0;
    let mut total_cost: u64 = 0;

    for step in steps {
        cumulative_delta += step.size as i128;

        // Compute margin based on net delta (delta-netting)
        let abs_delta = cumulative_delta.unsigned_abs();
        let primary_price = step.estimated_price as u128;
        let delta_notional = abs_delta * primary_price / precision;
        let margin = (delta_notional * config.initial_margin_bps as u128 / bps) as u64;

        // Apply credit discount
        let adjusted_margin = if config.credit_discount_bps > 0 {
            let maintenance = (margin as u128 * config.maintenance_ratio_bps as u128 / bps) as u64;
            tensor_math::apply_credit_discount(margin, config.credit_discount_bps, maintenance)
        } else {
            margin
        };

        if adjusted_margin > peak_margin {
            peak_margin = adjusted_margin;
        }

        let step_cost = (step.size.unsigned_abs() as u128 * step.estimated_price as u128 / precision) as u64;
        total_cost = total_cost.saturating_add(step_cost);
    }

    let feasible = peak_margin <= collateral;

    SolverResult {
        steps: steps.to_vec(),
        estimated_total_margin: peak_margin,
        estimated_total_cost: total_cost,
        feasible,
        reason: if feasible {
            None
        } else {
            Some(format!(
                "Peak margin {} exceeds collateral {}",
                peak_margin, collateral
            ))
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_leg(product: ProductType, market: u16, size: i64, price: u64) -> IntentLeg {
        IntentLeg {
            product_type: product,
            market_index: market,
            size,
            limit_price: price,
            is_active: true,
        }
    }

    // -----------------------------------------------------------------------
    // Decomposition tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_decompose_single_leg() {
        let bundle = IntentBundle::new().add_leg(make_leg(
            ProductType::Perpetual,
            0,
            10_000_000,
            150_000_000,
        ));
        let steps = decompose_intent(&bundle, &[]);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].size, 10_000_000);
        assert_eq!(steps[0].estimated_price, 150_000_000);
        assert_eq!(steps[0].product_type, ProductType::Perpetual);
    }

    #[test]
    fn test_decompose_multi_leg() {
        let bundle = IntentBundle::new()
            .add_leg(make_leg(ProductType::Spot, 0, 100_000_000, 0))
            .add_leg(make_leg(ProductType::Perpetual, 0, -100_000_000, 0));
        let prices = vec![150_000_000u64];
        let steps = decompose_intent(&bundle, &prices);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].leg_index, 0);
        assert_eq!(steps[1].leg_index, 1);
        // With limit_price=0, uses market price
        assert_eq!(steps[0].estimated_price, 150_000_000);
        assert_eq!(steps[1].estimated_price, 150_000_000);
    }

    #[test]
    fn test_decompose_uses_limit_price_when_set() {
        let bundle = IntentBundle::new()
            .add_leg(make_leg(ProductType::Perpetual, 0, 10_000_000, 140_000_000));
        let prices = vec![150_000_000u64];
        let steps = decompose_intent(&bundle, &prices);
        // Limit price takes precedence
        assert_eq!(steps[0].estimated_price, 140_000_000);
    }

    // -----------------------------------------------------------------------
    // Optimization tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_optimize_hedging_legs_first() {
        // Current portfolio is long delta
        let greeks = PortfolioGreeks {
            delta: 100_000_000, // long 100
            ..Default::default()
        };

        let mut steps = vec![
            ExecutionStep {
                leg_index: 0,
                product_type: ProductType::Perpetual,
                market_index: 0,
                size: 50_000_000, // long 50 (increases delta)
                estimated_price: 150_000_000,
                estimated_margin_delta: 7_500_000_000,
            },
            ExecutionStep {
                leg_index: 1,
                product_type: ProductType::Perpetual,
                market_index: 0,
                size: -80_000_000, // short 80 (reduces delta)
                estimated_price: 150_000_000,
                estimated_margin_delta: 12_000_000_000,
            },
        ];

        optimize_execution_order(&mut steps, &greeks);

        // Short leg should come first (reduces long delta)
        assert_eq!(steps[0].size, -80_000_000);
        assert_eq!(steps[1].size, 50_000_000);
    }

    #[test]
    fn test_optimize_delta_reducing_first_when_short() {
        // Current portfolio is short delta
        let greeks = PortfolioGreeks {
            delta: -50_000_000, // short 50
            ..Default::default()
        };

        let mut steps = vec![
            ExecutionStep {
                leg_index: 0,
                product_type: ProductType::Perpetual,
                market_index: 0,
                size: -10_000_000, // short (increases magnitude)
                estimated_price: 150_000_000,
                estimated_margin_delta: 1_500_000_000,
            },
            ExecutionStep {
                leg_index: 1,
                product_type: ProductType::Perpetual,
                market_index: 0,
                size: 30_000_000, // long (reduces magnitude)
                estimated_price: 150_000_000,
                estimated_margin_delta: 4_500_000_000,
            },
        ];

        optimize_execution_order(&mut steps, &greeks);

        // Long leg should come first (reduces short delta magnitude)
        assert_eq!(steps[0].size, 30_000_000);
        assert_eq!(steps[1].size, -10_000_000);
    }

    // -----------------------------------------------------------------------
    // Simulation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_simulate_feasible_trade() {
        let greeks = PortfolioGreeks::default();
        let steps = vec![ExecutionStep {
            leg_index: 0,
            product_type: ProductType::Perpetual,
            market_index: 0,
            size: 10_000_000,
            estimated_price: 150_000_000,
            estimated_margin_delta: 1_500_000_000,
        }];

        let result = simulate_margin_impact(
            &steps,
            &greeks,
            10_000_000_000, // $10,000 collateral
            &MarginSimConfig::default(),
        );

        assert!(result.feasible);
        assert!(result.reason.is_none());
        // margin = |10| * $150 * 10% = $150
        assert_eq!(result.estimated_total_margin, 150_000_000);
    }

    #[test]
    fn test_simulate_infeasible_exceeds_collateral() {
        let greeks = PortfolioGreeks::default();
        let steps = vec![ExecutionStep {
            leg_index: 0,
            product_type: ProductType::Perpetual,
            market_index: 0,
            size: 1_000_000_000, // 1000 units
            estimated_price: 150_000_000,
            estimated_margin_delta: 0,
        }];

        let result = simulate_margin_impact(
            &steps,
            &greeks,
            1_000_000_000, // $1,000 collateral
            &MarginSimConfig::default(),
        );

        assert!(!result.feasible);
        assert!(result.reason.is_some());
        // margin = |1000| * $150 * 10% = $15,000 > $1,000
        assert!(result.estimated_total_margin > 1_000_000_000);
    }

    #[test]
    fn test_simulate_credit_discount_applied() {
        let greeks = PortfolioGreeks::default();
        let steps = vec![ExecutionStep {
            leg_index: 0,
            product_type: ProductType::Perpetual,
            market_index: 0,
            size: 10_000_000,
            estimated_price: 150_000_000,
            estimated_margin_delta: 0,
        }];

        let config_no_credit = MarginSimConfig::default();
        let result_no_credit =
            simulate_margin_impact(&steps, &greeks, 10_000_000_000, &config_no_credit);

        let config_with_credit = MarginSimConfig {
            credit_discount_bps: 2000, // 20% Platinum
            ..Default::default()
        };
        let result_with_credit =
            simulate_margin_impact(&steps, &greeks, 10_000_000_000, &config_with_credit);

        // Credit discount should reduce margin
        assert!(result_with_credit.estimated_total_margin < result_no_credit.estimated_total_margin);
    }

    #[test]
    fn test_simulate_multi_leg_netting() {
        let greeks = PortfolioGreeks::default();

        // Long spot + short perp = delta-neutral → very low margin
        let steps = vec![
            ExecutionStep {
                leg_index: 0,
                product_type: ProductType::Spot,
                market_index: 0,
                size: 100_000_000,
                estimated_price: 150_000_000,
                estimated_margin_delta: 0,
            },
            ExecutionStep {
                leg_index: 1,
                product_type: ProductType::Perpetual,
                market_index: 0,
                size: -100_000_000,
                estimated_price: 150_000_000,
                estimated_margin_delta: 0,
            },
        ];

        let result = simulate_margin_impact(
            &steps,
            &greeks,
            10_000_000_000,
            &MarginSimConfig::default(),
        );

        assert!(result.feasible);
        // After both legs, net delta = 0, so final margin = 0
        // But peak margin was after first leg: |100| * $150 * 10% = $1500
        assert!(result.estimated_total_margin > 0); // peak is non-zero
    }

    #[test]
    fn test_margin_sim_config_defaults() {
        let config = MarginSimConfig::default();
        assert_eq!(config.initial_margin_bps, 1000);
        assert_eq!(config.maintenance_ratio_bps, 5000);
        assert_eq!(config.gamma_margin_bps, 100);
        assert_eq!(config.vega_margin_bps, 50);
        assert_eq!(config.credit_discount_bps, 0);
    }

    #[test]
    fn test_solver_result_feasibility_flag() {
        let result = SolverResult {
            steps: vec![],
            estimated_total_margin: 100,
            estimated_total_cost: 50,
            feasible: true,
            reason: None,
        };
        assert!(result.feasible);

        let result2 = SolverResult {
            steps: vec![],
            estimated_total_margin: 100,
            estimated_total_cost: 50,
            feasible: false,
            reason: Some("test".to_string()),
        };
        assert!(!result2.feasible);
        assert_eq!(result2.reason.as_deref(), Some("test"));
    }
}
