//! Tensor Math — Greeks-aware portfolio margin computation
//!
//! The key innovation: unified margin that accounts for delta-netting across
//! spot, perps, and options, plus gamma/vega adjustments for non-linear risk.

use tensor_types::*;

pub const PRECISION: u128 = 1_000_000;
pub const BPS: u128 = 10_000;

// ---------------------------------------------------------------------------
// Portfolio Margin Calculation
// ---------------------------------------------------------------------------

/// Compute aggregate portfolio Greeks from all position types.
///
/// This is the core of the unified margin engine. It sums deltas across
/// spot, perps, and options (with proper sign handling), then adds gamma
/// and vega contributions from options for the non-linear risk adjustment.
pub fn compute_portfolio_greeks(
    perp_positions: &[PerpPosition],
    spot_balances: &[SpotBalance],
    option_positions: &[OptionPosition],
    mark_prices: &[u64], // indexed by market_index
    current_time: i64,
) -> PortfolioGreeks {
    let mut delta: i128 = 0;
    let mut gamma: i128 = 0;
    let mut vega: i128 = 0;
    let mut theta: i128 = 0;
    let mut total_notional: u128 = 0;

    // Spot deltas (always long, 1:1 with underlying)
    for spot in spot_balances.iter().filter(|s| s.is_active) {
        delta += spot.balance as i128;
        total_notional += spot.value as u128;
    }

    // Perp deltas (signed, 1:1 with underlying)
    for perp in perp_positions.iter().filter(|p| p.is_active) {
        let price = mark_prices.get(perp.market_index as usize).copied().unwrap_or(0);
        delta += perp.size as i128;
        total_notional += perp.notional(price) as u128;
    }

    // Option Greeks (full non-linear contribution)
    for opt in option_positions.iter().filter(|o| o.is_active && o.expiry > current_time) {
        delta += opt.delta() as i128;
        gamma += opt.gamma() as i128;
        vega += opt.vega() as i128;
        theta += opt.theta() as i128;
        total_notional += opt.notional() as u128;
    }

    PortfolioGreeks {
        delta: delta as i64,
        gamma: gamma as i64,
        vega: vega as i64,
        theta: theta as i64,
        total_notional: total_notional as u64,
        computed_at: current_time,
    }
}

/// Compute initial margin requirement for a portfolio.
///
/// Formula:
///   base_margin = |net_delta| * mark_price * initial_margin_bps / BPS
///   gamma_charge = |gamma| * mark_price^2 * gamma_margin_bps / BPS^2
///   vega_charge  = |vega| * implied_vol * vega_margin_bps / BPS
///   total_margin = base_margin + gamma_charge + vega_charge
///
/// The delta-netting means that a hedged portfolio (e.g., long spot + short perp)
/// requires far less margin than the sum of individual position margins.
pub fn compute_initial_margin(
    greeks: &PortfolioGreeks,
    mark_price: u64,
    implied_vol_bps: u64,
    initial_margin_bps: u64,
    gamma_margin_bps: u64,
    vega_margin_bps: u64,
) -> u64 {
    // 1. Delta-based margin (main component)
    let abs_delta = if greeks.delta < 0 { -greeks.delta } else { greeks.delta } as u128;
    let delta_notional = abs_delta * mark_price as u128 / PRECISION;
    let delta_margin = delta_notional * initial_margin_bps as u128 / BPS;

    // 2. Gamma adjustment (non-linear risk from options)
    // Captures the risk that delta changes as price moves
    let abs_gamma = if greeks.gamma < 0 { -greeks.gamma } else { greeks.gamma } as u128;
    let gamma_charge = abs_gamma * (mark_price as u128).pow(2) * gamma_margin_bps as u128
        / (PRECISION * BPS * PRECISION);

    // 3. Vega adjustment (volatility risk from options + vol swaps)
    let abs_vega = if greeks.vega < 0 { -greeks.vega } else { greeks.vega } as u128;
    let vega_charge = abs_vega * implied_vol_bps as u128 * vega_margin_bps as u128 / (BPS * BPS);

    let total = delta_margin + gamma_charge + vega_charge;
    total as u64
}

/// Compute maintenance margin (typically 50-80% of initial margin).
pub fn compute_maintenance_margin(initial_margin: u64, maintenance_ratio_bps: u64) -> u64 {
    (initial_margin as u128 * maintenance_ratio_bps as u128 / BPS) as u64
}

// ---------------------------------------------------------------------------
// Equity Calculation
// ---------------------------------------------------------------------------

/// Total account equity = collateral + sum(unrealized PnL) + sum(lending value)
pub fn compute_equity(
    total_collateral: u64,
    perp_positions: &[PerpPosition],
    spot_balances: &[SpotBalance],
    option_positions: &[OptionPosition],
    lending_positions: &[LendingPosition],
    mark_prices: &[u64],
) -> i64 {
    let mut equity: i128 = total_collateral as i128;

    // Add perp unrealized PnL
    for perp in perp_positions.iter().filter(|p| p.is_active) {
        let price = mark_prices.get(perp.market_index as usize).copied().unwrap_or(0);
        equity += perp.mark_pnl(price) as i128;
        equity += perp.cumulative_funding as i128;
    }

    // Add spot value
    for spot in spot_balances.iter().filter(|s| s.is_active) {
        equity += spot.value as i128;
    }

    // Add option premium value (simplified: use premium paid as floor)
    for opt in option_positions.iter().filter(|o| o.is_active) {
        if opt.contracts > 0 {
            // Long option: adds intrinsic + time value (approximated by premium)
            equity += (opt.contracts as u128 * opt.premium as u128 / PRECISION) as i128;
        } else {
            // Short option: margin is already deducted, premium was received
            equity -= ((-opt.contracts) as u128 * opt.premium as u128 / PRECISION) as i128;
        }
    }

    // Add lending contributions
    for lending in lending_positions.iter().filter(|l| l.is_active) {
        equity += lending.margin_contribution() as i128;
    }

    equity as i64
}

// ---------------------------------------------------------------------------
// Health Check
// ---------------------------------------------------------------------------

/// Determine account health based on equity vs margin requirements.
pub fn compute_health(equity: i64, maintenance_margin: u64) -> AccountHealth {
    if equity <= 0 {
        return AccountHealth::Bankrupt;
    }
    if maintenance_margin == 0 {
        return AccountHealth::Healthy;
    }
    // margin_ratio = equity / maintenance_margin (in bps, 10000 = 1.0x)
    let ratio_bps = (equity as u128 * BPS) / maintenance_margin as u128;
    if ratio_bps <= BPS {
        AccountHealth::Liquidatable
    } else if ratio_bps <= 15_000 {
        // <= 1.5x
        AccountHealth::Warning
    } else {
        AccountHealth::Healthy
    }
}

/// Compute margin ratio in bps (10000 = equity equals maintenance margin)
pub fn margin_ratio_bps(equity: i64, maintenance_margin: u64) -> u16 {
    if maintenance_margin == 0 {
        return u16::MAX;
    }
    if equity <= 0 {
        return 0;
    }
    let ratio = (equity as u128 * BPS) / maintenance_margin as u128;
    ratio.min(u16::MAX as u128) as u16
}

// ---------------------------------------------------------------------------
// Collateral Haircut
// ---------------------------------------------------------------------------

/// Apply haircut to collateral value (same formula as northtail-collateral)
pub fn apply_haircut(value: u64, haircut_bps: u16) -> u64 {
    (value as u128 * (BPS - haircut_bps as u128) / BPS) as u64
}

// ---------------------------------------------------------------------------
// Liquidation Math
// ---------------------------------------------------------------------------

/// Calculate liquidation penalty
pub fn liquidation_fee(notional: u64, fee_bps: u64) -> u64 {
    (notional as u128 * fee_bps as u128 / BPS) as u64
}

/// Determine the liquidation waterfall priority.
/// Returns the product type to liquidate first.
///
/// Priority:
/// 1. Close expiring options (lowest time value remaining)
/// 2. Reduce perp positions (most liquid)
/// 3. Sell spot balances
/// 4. Seize lending collateral
pub fn liquidation_priority(
    perp_positions: &[PerpPosition],
    spot_balances: &[SpotBalance],
    option_positions: &[OptionPosition],
    lending_positions: &[LendingPosition],
    current_time: i64,
) -> Option<ProductType> {
    // Check for near-expiry options first
    let has_expiring_options = option_positions
        .iter()
        .any(|o| o.is_active && o.expiry > 0 && (o.expiry - current_time) < 86400);
    if has_expiring_options {
        return Some(ProductType::Option);
    }

    // Then perps (most liquid)
    let has_perps = perp_positions.iter().any(|p| p.is_active);
    if has_perps {
        return Some(ProductType::Perpetual);
    }

    // Then options (further-dated)
    let has_options = option_positions.iter().any(|o| o.is_active);
    if has_options {
        return Some(ProductType::Option);
    }

    // Then spot
    let has_spot = spot_balances.iter().any(|s| s.is_active);
    if has_spot {
        return Some(ProductType::Spot);
    }

    // Finally lending
    let has_lending = lending_positions.iter().any(|l| l.is_active);
    if has_lending {
        return Some(ProductType::Lending);
    }

    None
}

// ---------------------------------------------------------------------------
// Interest Rate Math (for lending)
// ---------------------------------------------------------------------------

/// Simple interest accrual: principal * rate_bps * elapsed_seconds / (365.25 * 86400 * 10000)
pub fn accrue_interest(principal: u64, rate_bps: u16, elapsed_seconds: i64) -> u64 {
    if elapsed_seconds <= 0 {
        return 0;
    }
    let seconds_per_year: u128 = 31_557_600; // 365.25 * 86400
    (principal as u128 * rate_bps as u128 * elapsed_seconds as u128
        / (seconds_per_year * BPS)) as u64
}

// ---------------------------------------------------------------------------
// NAV Calculation
// ---------------------------------------------------------------------------

/// Calculate NAV per share (compatible with northtail-vault)
pub fn calculate_nav(total_value: u64, total_shares: u64) -> u64 {
    if total_shares == 0 {
        return PRECISION as u64;
    }
    (total_value as u128 * PRECISION / total_shares as u128) as u64
}

/// Calculate shares for a deposit amount at given NAV
pub fn shares_for_deposit(amount: u64, nav_per_share: u64) -> u64 {
    if nav_per_share == 0 {
        return amount; // 1:1 if no NAV set
    }
    (amount as u128 * PRECISION / nav_per_share as u128) as u64
}

// ---------------------------------------------------------------------------
// ZK Credit Math (Phase 3)
// ---------------------------------------------------------------------------

/// Apply ZK credit discount to initial margin.
/// Returns margin * (BPS - discount_bps) / BPS, floored at maintenance margin.
pub fn apply_credit_discount(initial_margin: u64, discount_bps: u64, maintenance_margin: u64) -> u64 {
    if discount_bps == 0 || discount_bps >= BPS as u64 {
        return initial_margin;
    }
    let discounted = initial_margin
        .saturating_mul(BPS as u64 - discount_bps)
        / BPS as u64;
    discounted.max(maintenance_margin)
}

/// Effective max leverage with ZK credit bonus.
/// Returns base_leverage_bps + bonus_bps, capped at 100x (1_000_000 bps).
pub fn effective_max_leverage_bps(base_leverage_bps: u64, bonus_bps: u64) -> u64 {
    base_leverage_bps.saturating_add(bonus_bps).min(1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Delta-netting
    // -----------------------------------------------------------------------

    #[test]
    fn test_delta_netting() {
        // Long 100 SOL spot + short 100 SOL perp = net delta 0
        let mut perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        perps[0] = PerpPosition {
            market_index: 0,
            size: -100_000_000,
            entry_price: 150_000_000,
            is_active: true,
            ..Default::default()
        };

        let mut spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
        spots[0] = SpotBalance {
            balance: 100_000_000,
            value: 15_000_000_000,
            market_index: 0,
            is_active: true,
            ..Default::default()
        };

        let options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        let prices = vec![150_000_000u64];

        let greeks = compute_portfolio_greeks(&perps, &spots, &options, &prices, 0);
        assert_eq!(greeks.delta, 0);

        let margin = compute_initial_margin(&greeks, 150_000_000, 3000, 1000, 100, 50);
        assert_eq!(margin, 0);
    }

    #[test]
    fn test_delta_netting_partial_hedge() {
        // Long 100 spot, short 60 perp → net delta = 40
        let mut perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        perps[0] = PerpPosition {
            market_index: 0,
            size: -60_000_000,
            entry_price: 150_000_000,
            is_active: true,
            ..Default::default()
        };

        let mut spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
        spots[0] = SpotBalance {
            balance: 100_000_000,
            value: 15_000_000_000,
            market_index: 0,
            is_active: true,
            ..Default::default()
        };

        let options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        let prices = vec![150_000_000u64];
        let greeks = compute_portfolio_greeks(&perps, &spots, &options, &prices, 0);

        assert_eq!(greeks.delta, 40_000_000); // net long 40

        // Margin should be non-zero for unhedged portion
        let margin = compute_initial_margin(&greeks, 150_000_000, 3000, 1000, 100, 50);
        assert!(margin > 0);
    }

    // -----------------------------------------------------------------------
    // Gamma adjustment
    // -----------------------------------------------------------------------

    #[test]
    fn test_gamma_adjustment() {
        let perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        let spots = [SpotBalance::default(); MAX_SPOT_BALANCES];

        let mut options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        options[0] = OptionPosition {
            market_index: 0,
            side: OptionSide::Call,
            contracts: -10_000_000,
            notional_per_contract: 1_000_000,
            delta_per_contract: 500_000,
            gamma_per_contract: 50_000,
            vega_per_contract: 100_000,
            theta_per_contract: -20_000,
            expiry: 1_000_000,
            is_active: true,
            ..Default::default()
        };

        let prices = vec![150_000_000u64];
        let greeks = compute_portfolio_greeks(&perps, &spots, &options, &prices, 0);

        assert_eq!(greeks.delta, -5_000_000);
        assert_eq!(greeks.gamma, -500_000);

        let margin = compute_initial_margin(&greeks, 150_000_000, 3000, 1000, 100, 50);
        assert!(margin > 0);
    }

    #[test]
    fn test_gamma_neutral_requires_less_margin() {
        let perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        let spots = [SpotBalance::default(); MAX_SPOT_BALANCES];

        // Short gamma portfolio
        let mut opts_short = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        opts_short[0] = OptionPosition {
            contracts: -10_000_000,
            delta_per_contract: 0,
            gamma_per_contract: 50_000,
            vega_per_contract: 0,
            expiry: 1_000_000,
            notional_per_contract: 1_000_000,
            is_active: true,
            ..Default::default()
        };
        let greeks_short = compute_portfolio_greeks(&perps, &spots, &opts_short, &[150_000_000u64], 0);

        // Gamma-neutral portfolio (long + short cancel)
        let mut opts_neutral = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        opts_neutral[0] = OptionPosition {
            contracts: -10_000_000,
            delta_per_contract: 0,
            gamma_per_contract: 50_000,
            vega_per_contract: 0,
            expiry: 1_000_000,
            notional_per_contract: 1_000_000,
            is_active: true,
            ..Default::default()
        };
        opts_neutral[1] = OptionPosition {
            contracts: 10_000_000,
            delta_per_contract: 0,
            gamma_per_contract: 50_000,
            vega_per_contract: 0,
            expiry: 1_000_000,
            notional_per_contract: 1_000_000,
            is_active: true,
            ..Default::default()
        };
        let greeks_neutral = compute_portfolio_greeks(&perps, &spots, &opts_neutral, &[150_000_000u64], 0);

        let margin_short = compute_initial_margin(&greeks_short, 150_000_000, 3000, 1000, 100, 50);
        let margin_neutral = compute_initial_margin(&greeks_neutral, 150_000_000, 3000, 1000, 100, 50);

        assert_eq!(greeks_neutral.gamma, 0);
        assert!(margin_neutral < margin_short);
    }

    // -----------------------------------------------------------------------
    // Multi-product portfolio Greeks
    // -----------------------------------------------------------------------

    #[test]
    fn test_full_portfolio_greeks() {
        let mut perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        perps[0] = PerpPosition {
            market_index: 0,
            size: 50_000_000, // long 50
            entry_price: 150_000_000,
            is_active: true,
            ..Default::default()
        };

        let mut spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
        spots[0] = SpotBalance {
            balance: 30_000_000,
            value: 4_500_000_000,
            market_index: 0,
            is_active: true,
            ..Default::default()
        };

        let mut options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        options[0] = OptionPosition {
            contracts: -5_000_000,
            delta_per_contract: 600_000, // 0.6
            gamma_per_contract: 40_000,  // 0.04
            vega_per_contract: 150_000,  // 0.15
            theta_per_contract: -10_000,
            expiry: 1_000_000,
            notional_per_contract: 1_000_000,
            is_active: true,
            ..Default::default()
        };

        let prices = vec![150_000_000u64];
        let greeks = compute_portfolio_greeks(&perps, &spots, &options, &prices, 0);

        // delta: spot(30) + perp(50) + option(-5 * 0.6 = -3) = 77
        assert_eq!(greeks.delta, 77_000_000);
        // gamma: option(-5 * 0.04 = -0.2)
        assert_eq!(greeks.gamma, -200_000);
        // vega: option(-5 * 0.15 = -0.75)
        assert_eq!(greeks.vega, -750_000);
        // theta: option(-5 * -0.01 = 0.05) — wait let me recalculate
        // theta = -5_000_000 * -10_000 / 1_000_000 = 50_000
        assert_eq!(greeks.theta, 50_000);

        // total_notional: spot(4_500_000_000) + perp(50 * 150 = 7_500_000_000)
        //   + option(|5| * 1 = 5_000_000)
        assert_eq!(
            greeks.total_notional,
            4_500_000_000 + 7_500_000_000 + 5_000_000
        );
    }

    #[test]
    fn test_expired_options_excluded() {
        let perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        let spots = [SpotBalance::default(); MAX_SPOT_BALANCES];

        let mut options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        options[0] = OptionPosition {
            contracts: 10_000_000,
            delta_per_contract: 500_000,
            gamma_per_contract: 50_000,
            expiry: 100, // already expired when current_time >= 100
            notional_per_contract: 1_000_000,
            is_active: true,
            ..Default::default()
        };

        let greeks = compute_portfolio_greeks(&perps, &spots, &options, &[150_000_000u64], 200);

        // Expired option should not contribute
        assert_eq!(greeks.delta, 0);
        assert_eq!(greeks.gamma, 0);
        assert_eq!(greeks.total_notional, 0);
    }

    #[test]
    fn test_empty_portfolio() {
        let perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        let spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
        let options = [OptionPosition::default(); MAX_OPTION_POSITIONS];

        let greeks = compute_portfolio_greeks(&perps, &spots, &options, &[], 0);
        assert_eq!(greeks.delta, 0);
        assert_eq!(greeks.gamma, 0);
        assert_eq!(greeks.vega, 0);
        assert_eq!(greeks.theta, 0);
        assert_eq!(greeks.total_notional, 0);
    }

    // -----------------------------------------------------------------------
    // Initial margin formula verification
    // -----------------------------------------------------------------------

    #[test]
    fn test_margin_delta_only() {
        let greeks = PortfolioGreeks {
            delta: 100_000_000, // 100 units
            gamma: 0,
            vega: 0,
            theta: 0,
            total_notional: 0,
            computed_at: 0,
        };
        // base_margin = 100_000_000 * 150_000_000 / 1e6 * 1000 / 10000
        // = 15_000_000_000 * 0.1 = 1_500_000_000
        let margin = compute_initial_margin(&greeks, 150_000_000, 3000, 1000, 100, 50);
        assert_eq!(margin, 1_500_000_000);
    }

    #[test]
    fn test_margin_vega_only() {
        let greeks = PortfolioGreeks {
            delta: 0,
            gamma: 0,
            vega: 1_000_000, // 1.0 vega
            theta: 0,
            total_notional: 0,
            computed_at: 0,
        };
        // vega_charge = 1_000_000 * 3000 * 50 / (10000 * 10000)
        // = 150_000_000_000 / 100_000_000 = 1_500
        let margin = compute_initial_margin(&greeks, 150_000_000, 3000, 1000, 100, 50);
        assert_eq!(margin, 1_500);
    }

    // -----------------------------------------------------------------------
    // Maintenance margin
    // -----------------------------------------------------------------------

    #[test]
    fn test_maintenance_margin() {
        // 50% of initial
        assert_eq!(compute_maintenance_margin(1_000_000, 5000), 500_000);
        // 80% of initial
        assert_eq!(compute_maintenance_margin(1_000_000, 8000), 800_000);
        // 100% of initial
        assert_eq!(compute_maintenance_margin(1_000_000, 10000), 1_000_000);
    }

    // -----------------------------------------------------------------------
    // Health levels
    // -----------------------------------------------------------------------

    #[test]
    fn test_health_levels() {
        assert_eq!(compute_health(0, 100), AccountHealth::Bankrupt);
        assert_eq!(compute_health(-10, 100), AccountHealth::Bankrupt);
        assert_eq!(compute_health(100, 100), AccountHealth::Liquidatable);
        assert_eq!(compute_health(120, 100), AccountHealth::Warning);
        assert_eq!(compute_health(200, 100), AccountHealth::Healthy);
    }

    #[test]
    fn test_health_zero_margin() {
        // No positions → no margin requirement → healthy (if equity > 0)
        assert_eq!(compute_health(0, 0), AccountHealth::Bankrupt); // equity=0 is bankrupt
        assert_eq!(compute_health(100, 0), AccountHealth::Healthy);
    }

    #[test]
    fn test_health_boundary_values() {
        // Exactly at 1.0x (10000 bps) = Liquidatable
        assert_eq!(compute_health(10000, 10000), AccountHealth::Liquidatable);
        // Just above 1.0x = Warning
        assert_eq!(compute_health(10001, 10000), AccountHealth::Warning);
        // Exactly at 1.5x = Warning
        assert_eq!(compute_health(15000, 10000), AccountHealth::Warning);
        // Just above 1.5x = Healthy
        assert_eq!(compute_health(15001, 10000), AccountHealth::Healthy);
    }

    // -----------------------------------------------------------------------
    // Margin ratio
    // -----------------------------------------------------------------------

    #[test]
    fn test_margin_ratio_bps() {
        assert_eq!(margin_ratio_bps(10000, 10000), 10000); // 1.0x
        assert_eq!(margin_ratio_bps(20000, 10000), 20000); // 2.0x
        assert_eq!(margin_ratio_bps(5000, 10000), 5000);   // 0.5x
    }

    #[test]
    fn test_margin_ratio_no_margin() {
        assert_eq!(margin_ratio_bps(100, 0), u16::MAX);
    }

    #[test]
    fn test_margin_ratio_negative_equity() {
        assert_eq!(margin_ratio_bps(-100, 1000), 0);
        assert_eq!(margin_ratio_bps(0, 1000), 0);
    }

    // -----------------------------------------------------------------------
    // Haircuts
    // -----------------------------------------------------------------------

    #[test]
    fn test_haircut() {
        assert_eq!(apply_haircut(1_000_000, 500), 950_000);
        assert_eq!(apply_haircut(1_000_000, 0), 1_000_000);
        assert_eq!(apply_haircut(1_000_000, 2500), 750_000);
    }

    #[test]
    fn test_haircut_full() {
        assert_eq!(apply_haircut(1_000_000, 10000), 0); // 100% haircut
    }

    // -----------------------------------------------------------------------
    // Liquidation
    // -----------------------------------------------------------------------

    #[test]
    fn test_liquidation_fee() {
        assert_eq!(liquidation_fee(10_000_000, 50), 50_000); // 0.5%
        assert_eq!(liquidation_fee(10_000_000, 100), 100_000); // 1%
        assert_eq!(liquidation_fee(0, 100), 0);
    }

    #[test]
    fn test_liquidation_priority_near_expiry_options() {
        let perps = [PerpPosition { is_active: true, ..Default::default() }; MAX_PERP_POSITIONS];
        let spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
        let mut options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        options[0] = OptionPosition {
            expiry: 43200, // 12 hours from now (< 86400)
            is_active: true,
            ..Default::default()
        };
        let lending = [LendingPosition::default(); MAX_LENDING_POSITIONS];

        // At time 0, option expiry is 43200 (< 86400) → Options first
        let priority = liquidation_priority(&perps, &spots, &options, &lending, 0);
        assert_eq!(priority, Some(ProductType::Option));
    }

    #[test]
    fn test_liquidation_priority_perps_before_far_options() {
        let mut perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        perps[0].is_active = true;
        let spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
        let mut options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        options[0] = OptionPosition {
            expiry: 200_000, // far-dated (> 86400 from time 0)
            is_active: true,
            ..Default::default()
        };
        let lending = [LendingPosition::default(); MAX_LENDING_POSITIONS];

        // Far-dated options → perps come first
        let priority = liquidation_priority(&perps, &spots, &options, &lending, 0);
        assert_eq!(priority, Some(ProductType::Perpetual));
    }

    #[test]
    fn test_liquidation_priority_spot() {
        let perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        let mut spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
        spots[0].is_active = true;
        let options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        let lending = [LendingPosition::default(); MAX_LENDING_POSITIONS];

        let priority = liquidation_priority(&perps, &spots, &options, &lending, 0);
        assert_eq!(priority, Some(ProductType::Spot));
    }

    #[test]
    fn test_liquidation_priority_lending() {
        let perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        let spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
        let options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        let mut lending = [LendingPosition::default(); MAX_LENDING_POSITIONS];
        lending[0].is_active = true;

        let priority = liquidation_priority(&perps, &spots, &options, &lending, 0);
        assert_eq!(priority, Some(ProductType::Lending));
    }

    #[test]
    fn test_liquidation_priority_empty() {
        let perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        let spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
        let options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        let lending = [LendingPosition::default(); MAX_LENDING_POSITIONS];

        let priority = liquidation_priority(&perps, &spots, &options, &lending, 0);
        assert_eq!(priority, None);
    }

    // -----------------------------------------------------------------------
    // Equity calculation
    // -----------------------------------------------------------------------

    #[test]
    fn test_equity_collateral_only() {
        let perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        let spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
        let options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        let lending = [LendingPosition::default(); MAX_LENDING_POSITIONS];

        let equity = compute_equity(10_000_000_000, &perps, &spots, &options, &lending, &[]);
        assert_eq!(equity, 10_000_000_000);
    }

    #[test]
    fn test_equity_with_profitable_perp() {
        let mut perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        perps[0] = PerpPosition {
            market_index: 0,
            size: 10_000_000,      // long 10
            entry_price: 100_000_000, // $100
            cumulative_funding: 500_000, // $0.50 funding received
            is_active: true,
            ..Default::default()
        };
        let spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
        let options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        let lending = [LendingPosition::default(); MAX_LENDING_POSITIONS];

        // mark at $120: pnl = 10 * (120 - 100) = $200
        let prices = vec![120_000_000u64];
        let equity = compute_equity(1_000_000_000, &perps, &spots, &options, &lending, &prices);

        // 1_000_000_000 + 200_000_000 (pnl) + 500_000 (funding) = 1_200_500_000
        assert_eq!(equity, 1_200_500_000);
    }

    #[test]
    fn test_equity_with_losing_perp() {
        let mut perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        perps[0] = PerpPosition {
            market_index: 0,
            size: 10_000_000,
            entry_price: 100_000_000,
            is_active: true,
            ..Default::default()
        };
        let spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
        let options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        let lending = [LendingPosition::default(); MAX_LENDING_POSITIONS];

        // mark at $80: pnl = 10 * (80 - 100) = -$200
        let prices = vec![80_000_000u64];
        let equity = compute_equity(1_000_000_000, &perps, &spots, &options, &lending, &prices);
        assert_eq!(equity, 800_000_000);
    }

    #[test]
    fn test_equity_with_spot_and_lending() {
        let perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        let mut spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
        spots[0] = SpotBalance {
            balance: 10_000_000,
            value: 1_500_000_000, // $1500
            is_active: true,
            ..Default::default()
        };
        let options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        let mut lending = [LendingPosition::default(); MAX_LENDING_POSITIONS];
        lending[0] = LendingPosition {
            side: LendingSide::Supply,
            principal: 5_000_000_000,
            effective_value: 4_750_000_000, // after haircut
            is_active: true,
            ..Default::default()
        };
        lending[1] = LendingPosition {
            side: LendingSide::Borrow,
            principal: 2_000_000_000,
            accrued_interest: 50_000_000,
            is_active: true,
            ..Default::default()
        };

        let equity = compute_equity(
            5_000_000_000,
            &perps, &spots, &options, &lending, &[],
        );
        // 5B + 1.5B (spot) + 4.75B (supply) - (2B + 0.05B) (borrow) = 9.2B
        assert_eq!(
            equity,
            5_000_000_000 + 1_500_000_000 + 4_750_000_000 - 2_000_000_000 - 50_000_000
        );
    }

    #[test]
    fn test_equity_with_long_option() {
        let perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        let spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
        let mut options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        options[0] = OptionPosition {
            contracts: 5_000_000, // long 5
            premium: 2_000_000,   // $2 premium
            is_active: true,
            expiry: 1_000_000,
            ..Default::default()
        };
        let lending = [LendingPosition::default(); MAX_LENDING_POSITIONS];

        let equity = compute_equity(1_000_000_000, &perps, &spots, &options, &lending, &[]);
        // collateral + long option value = 1B + (5 * 2 / 1e6 * 1e6) = 1B + 10
        // 5_000_000 * 2_000_000 / 1_000_000 = 10_000_000
        assert_eq!(equity, 1_010_000_000);
    }

    // -----------------------------------------------------------------------
    // Interest accrual
    // -----------------------------------------------------------------------

    #[test]
    fn test_interest_accrual() {
        let interest = accrue_interest(1_000_000_000, 500, 31_557_600);
        assert!((interest as i64 - 50_000_000).abs() < 100_000);
    }

    #[test]
    fn test_interest_accrual_zero_elapsed() {
        assert_eq!(accrue_interest(1_000_000_000, 500, 0), 0);
    }

    #[test]
    fn test_interest_accrual_negative_elapsed() {
        assert_eq!(accrue_interest(1_000_000_000, 500, -100), 0);
    }

    #[test]
    fn test_interest_accrual_zero_principal() {
        assert_eq!(accrue_interest(0, 500, 31_557_600), 0);
    }

    #[test]
    fn test_interest_accrual_one_day() {
        // $1000 at 10% for 1 day
        let interest = accrue_interest(1_000_000_000, 1000, 86400);
        // Expected: ~$0.2739 per day ≈ 273_972
        assert!(interest > 0);
        assert!((interest as i64 - 273_972).abs() < 10_000);
    }

    // -----------------------------------------------------------------------
    // NAV calculation
    // -----------------------------------------------------------------------

    #[test]
    fn test_nav_calculation() {
        // 1:1 NAV
        assert_eq!(calculate_nav(1_000_000, 1_000_000), 1_000_000);
        // $2 per share
        assert_eq!(calculate_nav(2_000_000, 1_000_000), 2_000_000);
    }

    #[test]
    fn test_nav_zero_shares() {
        // No shares → NAV = 1.0 (PRECISION)
        assert_eq!(calculate_nav(0, 0), 1_000_000);
    }

    #[test]
    fn test_shares_for_deposit() {
        // $100 at $1 NAV = 100 shares
        assert_eq!(shares_for_deposit(100_000_000, 1_000_000), 100_000_000);
        // $100 at $2 NAV = 50 shares
        assert_eq!(shares_for_deposit(100_000_000, 2_000_000), 50_000_000);
    }

    #[test]
    fn test_shares_for_deposit_zero_nav() {
        assert_eq!(shares_for_deposit(100_000_000, 0), 100_000_000);
    }

    // -----------------------------------------------------------------------
    // End-to-end margin scenarios
    // -----------------------------------------------------------------------

    #[test]
    fn test_scenario_levered_long_healthy() {
        // $1000 collateral, long 10 SOL at $150 = $1500 notional = 1.5x leverage
        // At 10% initial margin → $150 margin needed → healthy
        let mut perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        perps[0] = PerpPosition {
            market_index: 0,
            size: 10_000_000,
            entry_price: 150_000_000,
            is_active: true,
            ..Default::default()
        };
        let spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
        let options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        let lending = [LendingPosition::default(); MAX_LENDING_POSITIONS];
        let prices = vec![150_000_000u64];

        let greeks = compute_portfolio_greeks(&perps, &spots, &options, &prices, 0);
        let initial_margin = compute_initial_margin(&greeks, 150_000_000, 3000, 1000, 100, 50);
        let maint_margin = compute_maintenance_margin(initial_margin, 5000);
        let equity = compute_equity(1_000_000_000, &perps, &spots, &options, &lending, &prices);
        let health = compute_health(equity, maint_margin);

        assert_eq!(greeks.delta, 10_000_000);
        assert_eq!(initial_margin, 150_000_000); // $150
        assert_eq!(maint_margin, 75_000_000);    // $75
        assert_eq!(equity, 1_000_000_000);       // $1000
        assert_eq!(health, AccountHealth::Healthy);
    }

    #[test]
    fn test_scenario_underwater_short() {
        // $200 collateral, short 10 SOL at $100, price rises to $130
        // PnL = -10 * (130 - 100) = -$300 → equity = 200 - 300 = -100 → bankrupt
        let mut perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        perps[0] = PerpPosition {
            market_index: 0,
            size: -10_000_000,
            entry_price: 100_000_000,
            is_active: true,
            ..Default::default()
        };
        let spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
        let options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        let lending = [LendingPosition::default(); MAX_LENDING_POSITIONS];
        let prices = vec![130_000_000u64];

        let greeks = compute_portfolio_greeks(&perps, &spots, &options, &prices, 0);
        let initial_margin = compute_initial_margin(&greeks, 130_000_000, 3000, 1000, 100, 50);
        let maint_margin = compute_maintenance_margin(initial_margin, 5000);
        let equity = compute_equity(200_000_000, &perps, &spots, &options, &lending, &prices);
        let health = compute_health(equity, maint_margin);

        assert_eq!(equity, -100_000_000);
        assert_eq!(health, AccountHealth::Bankrupt);
    }

    #[test]
    fn test_scenario_delta_neutral_with_options() {
        // Perfect hedge: long 100 SOL spot, short 100 SOL perp, plus short 5 calls
        // Delta should be near-zero from spot+perp, options add gamma/vega risk
        let mut perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
        perps[0] = PerpPosition {
            market_index: 0,
            size: -100_000_000,
            entry_price: 150_000_000,
            is_active: true,
            ..Default::default()
        };

        let mut spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
        spots[0] = SpotBalance {
            balance: 100_000_000,
            value: 15_000_000_000,
            is_active: true,
            ..Default::default()
        };

        let mut options = [OptionPosition::default(); MAX_OPTION_POSITIONS];
        options[0] = OptionPosition {
            contracts: -5_000_000,
            delta_per_contract: 500_000,
            gamma_per_contract: 80_000,
            vega_per_contract: 200_000,
            expiry: 1_000_000,
            notional_per_contract: 1_000_000,
            is_active: true,
            ..Default::default()
        };

        let prices = vec![150_000_000u64];
        let greeks = compute_portfolio_greeks(&perps, &spots, &options, &prices, 0);

        // spot(100) + perp(-100) + option(-2.5) = -2.5
        assert_eq!(greeks.delta, -2_500_000);
        // Gamma from options only
        assert_eq!(greeks.gamma, -400_000);

        let margin = compute_initial_margin(&greeks, 150_000_000, 3000, 1000, 100, 50);
        // Margin breakdown:
        // delta margin: |2.5| * $150 * 10% = $37.50 → 37_500_000
        // gamma charge: |0.4| * $150^2 * 1% = $90 → 90_000_000
        // vega charge: |1.0| * 3000 * 0.5% = $0.0015 → 1_500
        // Total ≈ $127.50
        assert!(margin > 0);
        assert!(margin > 100_000_000); // gamma dominates
        assert!(margin < 200_000_000); // but still moderate
    }

    // -----------------------------------------------------------------------
    // Credit discount tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_credit_discount_zero() {
        assert_eq!(apply_credit_discount(1_000_000, 0, 500_000), 1_000_000);
    }

    #[test]
    fn test_credit_discount_bronze() {
        // 5% discount: 1_000_000 * 9500 / 10000 = 950_000
        assert_eq!(apply_credit_discount(1_000_000, 500, 400_000), 950_000);
    }

    #[test]
    fn test_credit_discount_silver() {
        // 10% discount: 1_000_000 * 9000 / 10000 = 900_000
        assert_eq!(apply_credit_discount(1_000_000, 1000, 400_000), 900_000);
    }

    #[test]
    fn test_credit_discount_gold() {
        // 15% discount: 1_000_000 * 8500 / 10000 = 850_000
        assert_eq!(apply_credit_discount(1_000_000, 1500, 400_000), 850_000);
    }

    #[test]
    fn test_credit_discount_platinum() {
        // 20% discount: 1_000_000 * 8000 / 10000 = 800_000
        assert_eq!(apply_credit_discount(1_000_000, 2000, 400_000), 800_000);
    }

    #[test]
    fn test_credit_discount_floored_at_maintenance() {
        // 20% discount would give 800_000 but maintenance is 900_000
        assert_eq!(apply_credit_discount(1_000_000, 2000, 900_000), 900_000);
    }

    #[test]
    fn test_credit_discount_full_discount_returns_initial() {
        // 100% discount (>= BPS) returns initial margin unchanged
        assert_eq!(apply_credit_discount(1_000_000, 10000, 500_000), 1_000_000);
        assert_eq!(apply_credit_discount(1_000_000, 15000, 500_000), 1_000_000);
    }

    // -----------------------------------------------------------------------
    // Effective leverage tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_effective_leverage_no_bonus() {
        assert_eq!(effective_max_leverage_bps(50_000, 0), 50_000);
    }

    #[test]
    fn test_effective_leverage_with_bonus() {
        // Retail 5x + Bronze 0.25x = 5.25x
        assert_eq!(effective_max_leverage_bps(50_000, 2500), 52_500);
    }

    #[test]
    fn test_effective_leverage_capped_at_100x() {
        // Institutional 50x + Platinum 1x = 51x (under cap)
        assert_eq!(effective_max_leverage_bps(500_000, 10000), 510_000);
        // Some huge value should cap at 100x = 1_000_000
        assert_eq!(effective_max_leverage_bps(900_000, 200_000), 1_000_000);
    }
}
