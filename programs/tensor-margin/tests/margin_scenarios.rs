//! End-to-end margin scenario tests.
//!
//! These tests simulate full trading workflows by constructing state structs
//! and running the same computations that the on-chain instructions use.
//! They verify the business logic without needing a Solana validator.

use tensor_types::*;

#[allow(unused_imports)]
use tensor_intents;
#[allow(unused_imports)]
use tensor_solver;

const PRECISION: u128 = 1_000_000;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Simulated margin account state
struct SimAccount {
    collateral: u64,
    locked_collateral: u64,
    perp_positions: [PerpPosition; MAX_PERP_POSITIONS],
    perp_count: u8,
    spot_balances: [SpotBalance; MAX_SPOT_BALANCES],
    spot_count: u8,
    option_positions: [OptionPosition; MAX_OPTION_POSITIONS],
    option_count: u8,
    lending_positions: [LendingPosition; MAX_LENDING_POSITIONS],
    lending_count: u8,
    investor_category: InvestorCategory,
    total_trades: u64,
    total_realized_pnl: i64,
}

impl SimAccount {
    fn new(collateral: u64) -> Self {
        Self {
            collateral,
            locked_collateral: 0,
            perp_positions: [PerpPosition::default(); MAX_PERP_POSITIONS],
            perp_count: 0,
            spot_balances: [SpotBalance::default(); MAX_SPOT_BALANCES],
            spot_count: 0,
            option_positions: [OptionPosition::default(); MAX_OPTION_POSITIONS],
            option_count: 0,
            lending_positions: [LendingPosition::default(); MAX_LENDING_POSITIONS],
            lending_count: 0,
            investor_category: InvestorCategory::Retail,
            total_trades: 0,
            total_realized_pnl: 0,
        }
    }

    fn open_perp(&mut self, market_index: u16, size: i64, entry_price: u64) -> Result<(), &str> {
        if size == 0 {
            return Err("zero size");
        }
        let slot = self.perp_positions.iter().position(|p| !p.is_active)
            .ok_or("slot full")?;
        self.perp_positions[slot] = PerpPosition {
            market_index,
            size,
            entry_price,
            realized_pnl: 0,
            unrealized_pnl: 0,
            cumulative_funding: 0,
            last_funding_index: 0,
            opened_at: 0,
            is_active: true,
        };
        self.perp_count += 1;
        self.total_trades += 1;
        Ok(())
    }

    fn close_perp(&mut self, market_index: u16, mark_price: u64) -> Result<i64, &str> {
        let idx = self.perp_positions.iter().position(|p| p.is_active && p.market_index == market_index)
            .ok_or("not found")?;

        let pnl = self.perp_positions[idx].mark_pnl(mark_price);
        let total_pnl = pnl + self.perp_positions[idx].cumulative_funding;

        if total_pnl > 0 {
            self.collateral = self.collateral.saturating_add(total_pnl as u64);
        } else {
            self.collateral = self.collateral.saturating_sub((-total_pnl) as u64);
        }
        self.total_realized_pnl += pnl;

        self.perp_positions[idx] = PerpPosition::default();
        self.perp_count -= 1;
        Ok(pnl)
    }

    fn add_spot(&mut self, market_index: u16, balance: u64, value: u64) -> Result<(), &str> {
        let slot = self.spot_balances.iter().position(|s| !s.is_active)
            .ok_or("slot full")?;
        self.spot_balances[slot] = SpotBalance {
            mint: anchor_lang::prelude::Pubkey::new_unique(),
            balance,
            value,
            market_index,
            is_active: true,
        };
        self.spot_count += 1;
        Ok(())
    }

    fn open_option(
        &mut self,
        market_index: u16,
        side: OptionSide,
        contracts: i64,
        strike: u64,
        premium: u64,
        delta: i64,
        gamma: i64,
        vega: i64,
        theta: i64,
        notional_per_contract: u64,
        expiry: i64,
    ) -> Result<(), &str> {
        let slot = self.option_positions.iter().position(|o| !o.is_active)
            .ok_or("slot full")?;

        let abs_contracts = contracts.unsigned_abs() as u128;
        let total_premium = (abs_contracts * premium as u128 / PRECISION) as u64;

        if contracts > 0 {
            if self.collateral < total_premium {
                return Err("insufficient collateral");
            }
            self.collateral -= total_premium;
        } else {
            self.collateral += total_premium;
        }

        self.option_positions[slot] = OptionPosition {
            market_index,
            side,
            kind: OptionKind::Vanilla,
            strike,
            barrier: 0,
            contracts,
            notional_per_contract,
            expiry,
            premium,
            delta_per_contract: delta,
            gamma_per_contract: gamma,
            vega_per_contract: vega,
            theta_per_contract: theta,
            opened_at: 0,
            is_active: true,
        };
        self.option_count += 1;
        self.total_trades += 1;
        Ok(())
    }

    fn add_lending(
        &mut self,
        lside: LendingSide,
        principal: u64,
        effective_value: u64,
        rate_bps: u16,
    ) -> Result<(), &str> {
        let slot = self.lending_positions.iter().position(|l| !l.is_active)
            .ok_or("slot full")?;
        self.lending_positions[slot] = LendingPosition {
            mint: anchor_lang::prelude::Pubkey::new_unique(),
            market_index: 0,
            side: lside,
            principal,
            accrued_interest: 0,
            rate_bps,
            haircut_bps: if matches!(lside, LendingSide::Supply) { 500 } else { 0 },
            effective_value,
            last_accrual: 0,
            is_active: true,
        };
        self.lending_count += 1;
        Ok(())
    }

    fn compute_margin(
        &self,
        mark_prices: &[u64],
        current_time: i64,
        initial_margin_bps: u64,
        maintenance_ratio_bps: u64,
        gamma_margin_bps: u64,
        vega_margin_bps: u64,
        implied_vol_bps: u64,
    ) -> MarginResult {
        let greeks = tensor_math::compute_portfolio_greeks(
            &self.perp_positions,
            &self.spot_balances,
            &self.option_positions,
            mark_prices,
            current_time,
        );

        let primary_price = mark_prices.first().copied().unwrap_or(0);

        let initial_margin = tensor_math::compute_initial_margin(
            &greeks,
            primary_price,
            implied_vol_bps,
            initial_margin_bps,
            gamma_margin_bps,
            vega_margin_bps,
        );

        let maint_margin = tensor_math::compute_maintenance_margin(
            initial_margin,
            maintenance_ratio_bps,
        );

        let equity = tensor_math::compute_equity(
            self.collateral,
            &self.perp_positions,
            &self.spot_balances,
            &self.option_positions,
            &self.lending_positions,
            mark_prices,
        );

        let health = tensor_math::compute_health(equity, maint_margin);
        let ratio = tensor_math::margin_ratio_bps(equity, maint_margin);

        // Check leverage
        let leverage_bps = if self.collateral > 0 && greeks.total_notional > 0 {
            (greeks.total_notional as u128 * 10_000) / self.collateral as u128
        } else {
            0
        };

        MarginResult {
            greeks,
            initial_margin,
            maintenance_margin: maint_margin,
            equity,
            health,
            margin_ratio_bps: ratio,
            leverage_bps: leverage_bps as u64,
        }
    }
}

struct MarginResult {
    greeks: PortfolioGreeks,
    initial_margin: u64,
    maintenance_margin: u64,
    equity: i64,
    health: AccountHealth,
    margin_ratio_bps: u16,
    leverage_bps: u64,
}

// Standard config: 10% IM, 50% maintenance ratio, 1% gamma, 0.5% vega
const IM_BPS: u64 = 1000;
const MAINT_RATIO: u64 = 5000;
const GAMMA_BPS: u64 = 100;
const VEGA_BPS: u64 = 50;
const IMPLIED_VOL: u64 = 3000;

// =======================================================================
// Scenario 1: Simple long perp — deposit, trade, profit, close
// =======================================================================

#[test]
fn test_scenario_simple_long_perp_lifecycle() {
    let mut acc = SimAccount::new(10_000_000_000); // $10,000

    // Open long 10 SOL at $150
    acc.open_perp(0, 10_000_000, 150_000_000).unwrap();
    assert_eq!(acc.perp_count, 1);
    assert_eq!(acc.total_trades, 1);

    // Check margin at entry
    let mr = acc.compute_margin(&[150_000_000], 0, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);
    assert_eq!(mr.greeks.delta, 10_000_000);
    // Initial margin: 10 * $150 * 10% = $150
    assert_eq!(mr.initial_margin, 150_000_000);
    assert_eq!(mr.equity, 10_000_000_000);
    assert_eq!(mr.health, AccountHealth::Healthy);

    // Price rises to $200
    let mr2 = acc.compute_margin(&[200_000_000], 100, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);
    // Unrealized PnL: 10 * (200 - 150) = $500
    assert_eq!(mr2.equity, 10_500_000_000);
    assert_eq!(mr2.health, AccountHealth::Healthy);

    // Close at $200
    let pnl = acc.close_perp(0, 200_000_000).unwrap();
    assert_eq!(pnl, 500_000_000); // $500 profit
    assert_eq!(acc.collateral, 10_500_000_000);
    assert_eq!(acc.perp_count, 0);
}

// =======================================================================
// Scenario 2: Short perp — loss leading to liquidation
// =======================================================================

#[test]
fn test_scenario_short_perp_liquidation() {
    let mut acc = SimAccount::new(500_000_000); // $500

    // Short 10 SOL at $100
    acc.open_perp(0, -10_000_000, 100_000_000).unwrap();

    // At entry: equity = $500, margin needed = 10 * $100 * 10% = $100
    let mr = acc.compute_margin(&[100_000_000], 0, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);
    assert_eq!(mr.equity, 500_000_000);
    assert_eq!(mr.health, AccountHealth::Healthy);

    // Price rises to $140 → PnL = -10 * (140 - 100) = -$400
    // Equity = 500 - 400 = $100
    // Maintenance margin = 10 * $140 * 10% * 50% = $70
    // Ratio = 100/70 ≈ 1.43x → Warning zone
    let mr2 = acc.compute_margin(&[140_000_000], 100, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);
    assert_eq!(mr2.equity, 100_000_000);
    assert_eq!(mr2.health, AccountHealth::Warning);

    // Price rises to $150 → PnL = -$500 → equity = 0 → Bankrupt
    let mr3 = acc.compute_margin(&[150_000_000], 200, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);
    assert_eq!(mr3.equity, 0);
    assert_eq!(mr3.health, AccountHealth::Bankrupt);
}

// =======================================================================
// Scenario 3: Delta-neutral portfolio (cash & carry)
// =======================================================================

#[test]
fn test_scenario_cash_and_carry_delta_neutral() {
    let mut acc = SimAccount::new(20_000_000_000); // $20,000

    // Long 100 SOL spot at $150
    acc.add_spot(0, 100_000_000, 15_000_000_000).unwrap();

    // Short 100 SOL perp at $150
    acc.open_perp(0, -100_000_000, 150_000_000).unwrap();

    // Delta should net to 0
    let mr = acc.compute_margin(&[150_000_000], 0, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);
    assert_eq!(mr.greeks.delta, 0);
    assert_eq!(mr.initial_margin, 0); // No delta margin needed!
    assert_eq!(mr.health, AccountHealth::Healthy);

    // Price moves to $200 — delta-neutral means no PnL impact from delta
    let mr2 = acc.compute_margin(&[200_000_000], 100, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);
    assert_eq!(mr2.greeks.delta, 0); // Still neutral
    // Equity changes from:
    // - Perp PnL: -100 * (200 - 150) = -$5000
    // - Spot value: still counted at original value (15B) since compute_equity uses spot.value
    // But collateral stays at 20B, spot value at 15B, perp PnL = -5B, so equity = 20B + 15B - 5B = 30B
    assert_eq!(mr2.equity, 30_000_000_000);
}

// =======================================================================
// Scenario 4: Options with Greeks (gamma/vega risk)
// =======================================================================

#[test]
fn test_scenario_short_straddle() {
    let mut acc = SimAccount::new(50_000_000_000); // $50,000

    // Short 10 ATM calls: delta=0.5, gamma=0.05, vega=0.15
    acc.open_option(
        0, OptionSide::Call,
        -10_000_000, // short 10
        150_000_000, // $150 strike
        5_000_000,   // $5 premium per contract
        500_000,     // 0.5 delta
        50_000,      // 0.05 gamma
        150_000,     // 0.15 vega
        -30_000,     // -0.03 theta
        1_000_000,   // $1 notional per contract
        1_000_000,   // far expiry
    ).unwrap();

    // Short 10 ATM puts: delta=-0.5, gamma=0.05, vega=0.15
    acc.open_option(
        0, OptionSide::Put,
        -10_000_000,
        150_000_000,
        4_500_000,   // $4.50 premium
        -500_000,    // -0.5 delta (put)
        50_000,      // 0.05 gamma
        150_000,     // 0.15 vega
        -25_000,
        1_000_000,
        1_000_000,
    ).unwrap();

    // Straddle: delta should cancel (short call delta + short put delta)
    // call: -10 * 0.5 = -5
    // put:  -10 * -0.5 = +5
    // net delta = 0
    let mr = acc.compute_margin(&[150_000_000], 0, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);
    assert_eq!(mr.greeks.delta, 0); // delta-neutral straddle

    // But gamma and vega should be significant (doubled)
    assert_eq!(mr.greeks.gamma, -1_000_000); // -0.5 + -0.5 = -1.0
    assert_eq!(mr.greeks.vega, -3_000_000);  // -1.5 + -1.5 = -3.0

    // Margin should come entirely from gamma/vega charges
    assert!(mr.initial_margin > 0);
    assert_eq!(mr.health, AccountHealth::Healthy);
}

// =======================================================================
// Scenario 5: Multi-product portfolio
// =======================================================================

#[test]
fn test_scenario_multi_product_portfolio() {
    let mut acc = SimAccount::new(100_000_000_000); // $100,000

    // Long 50 SOL spot ($7500)
    acc.add_spot(0, 50_000_000, 7_500_000_000).unwrap();

    // Short 30 SOL perp (hedge part of spot)
    acc.open_perp(0, -30_000_000, 150_000_000).unwrap();

    // Long 5 call options for upside
    acc.open_option(
        0, OptionSide::Call,
        5_000_000, 160_000_000, 3_000_000,
        400_000, 30_000, 100_000, -15_000,
        1_000_000, 1_000_000,
    ).unwrap();

    // Supply $10,000 in lending (earns yield)
    acc.add_lending(LendingSide::Supply, 10_000_000_000, 9_500_000_000, 500).unwrap();

    let mr = acc.compute_margin(&[150_000_000], 0, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);

    // Net delta: spot(50) + perp(-30) + option(5 * 0.4 = 2) = 22
    assert_eq!(mr.greeks.delta, 22_000_000);

    // Has all 4 product types active
    assert_eq!(acc.perp_count, 1);
    assert_eq!(acc.spot_count, 1);
    assert_eq!(acc.option_count, 1);
    assert_eq!(acc.lending_count, 1);

    assert_eq!(mr.health, AccountHealth::Healthy);
}

// =======================================================================
// Scenario 6: Leverage limits by investor category
// =======================================================================

#[test]
fn test_scenario_leverage_limits() {
    // Retail: 5x max
    let mut retail = SimAccount::new(1_000_000_000); // $1000
    retail.investor_category = InvestorCategory::Retail;
    retail.open_perp(0, 30_000_000, 150_000_000).unwrap(); // $4500 notional

    let mr = retail.compute_margin(&[150_000_000], 0, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);
    // Leverage = $4500 / $1000 = 4.5x = 45000 bps
    assert_eq!(mr.leverage_bps, 45_000);
    assert!(mr.leverage_bps <= InvestorCategory::Retail.max_leverage_bps());

    // Try 6x leverage (would be rejected on-chain)
    let mut over_levered = SimAccount::new(1_000_000_000);
    over_levered.investor_category = InvestorCategory::Retail;
    over_levered.open_perp(0, 40_000_000, 150_000_000).unwrap(); // $6000 = 6x

    let mr2 = over_levered.compute_margin(&[150_000_000], 0, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);
    assert_eq!(mr2.leverage_bps, 60_000); // 6x
    assert!(mr2.leverage_bps > InvestorCategory::Retail.max_leverage_bps()); // over limit!
    assert!(mr2.leverage_bps <= InvestorCategory::Qualified.max_leverage_bps()); // but OK for qualified
}

// =======================================================================
// Scenario 7: Position slot limits
// =======================================================================

#[test]
fn test_scenario_max_perp_positions() {
    let mut acc = SimAccount::new(100_000_000_000);

    // Fill all 8 perp slots
    for i in 0..MAX_PERP_POSITIONS {
        acc.open_perp(i as u16, 1_000_000, 100_000_000).unwrap();
    }
    assert_eq!(acc.perp_count, MAX_PERP_POSITIONS as u8);

    // 9th should fail
    assert!(acc.open_perp(100, 1_000_000, 100_000_000).is_err());
}

#[test]
fn test_scenario_max_option_positions() {
    let mut acc = SimAccount::new(100_000_000_000);

    for i in 0..MAX_OPTION_POSITIONS {
        acc.open_option(
            i as u16, OptionSide::Call, 1_000_000, 100_000_000,
            1_000_000, 500_000, 50_000, 100_000, -10_000,
            1_000_000, 1_000_000,
        ).unwrap();
    }
    assert_eq!(acc.option_count, MAX_OPTION_POSITIONS as u8);

    assert!(acc.open_option(
        100, OptionSide::Call, 1_000_000, 100_000_000,
        1_000_000, 500_000, 50_000, 100_000, -10_000,
        1_000_000, 1_000_000,
    ).is_err());
}

// =======================================================================
// Scenario 8: Liquidation waterfall
// =======================================================================

#[test]
fn test_scenario_liquidation_waterfall() {
    let mut acc = SimAccount::new(100_000_000); // $100

    // Near-expiry option (< 86400 seconds)
    acc.open_option(
        0, OptionSide::Call, -1_000_000, 100_000_000,
        1_000_000, 500_000, 50_000, 100_000, -10_000,
        1_000_000, 50_000, // expiry in 50000 seconds
    ).unwrap();

    // Active perp
    acc.open_perp(0, 1_000_000, 100_000_000).unwrap();

    // Spot balance
    acc.add_spot(0, 1_000_000, 100_000_000).unwrap();

    // Lending supply
    acc.add_lending(LendingSide::Supply, 50_000_000, 47_500_000, 500).unwrap();

    // Waterfall priority at time 0: near-expiry option first
    let priority = tensor_math::liquidation_priority(
        &acc.perp_positions,
        &acc.spot_balances,
        &acc.option_positions,
        &acc.lending_positions,
        0,
    );
    assert_eq!(priority, Some(ProductType::Option));

    // After removing options: perps next
    let mut no_options = acc.option_positions;
    for o in no_options.iter_mut() {
        *o = OptionPosition::default();
    }
    let priority2 = tensor_math::liquidation_priority(
        &acc.perp_positions,
        &acc.spot_balances,
        &no_options,
        &acc.lending_positions,
        0,
    );
    assert_eq!(priority2, Some(ProductType::Perpetual));

    // After removing perps: spot next
    let no_perps = [PerpPosition::default(); MAX_PERP_POSITIONS];
    let priority3 = tensor_math::liquidation_priority(
        &no_perps,
        &acc.spot_balances,
        &no_options,
        &acc.lending_positions,
        0,
    );
    assert_eq!(priority3, Some(ProductType::Spot));

    // After removing spot: lending last
    let no_spots = [SpotBalance::default(); MAX_SPOT_BALANCES];
    let priority4 = tensor_math::liquidation_priority(
        &no_perps,
        &no_spots,
        &no_options,
        &acc.lending_positions,
        0,
    );
    assert_eq!(priority4, Some(ProductType::Lending));
}

// =======================================================================
// Scenario 9: Closing positions releases margin
// =======================================================================

#[test]
fn test_scenario_close_releases_margin() {
    let mut acc = SimAccount::new(10_000_000_000);

    // Open position
    acc.open_perp(0, 50_000_000, 150_000_000).unwrap();

    let mr_before = acc.compute_margin(&[150_000_000], 0, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);
    assert!(mr_before.initial_margin > 0);

    // Close position at same price (no PnL)
    acc.close_perp(0, 150_000_000).unwrap();

    let mr_after = acc.compute_margin(&[150_000_000], 100, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);
    assert_eq!(mr_after.initial_margin, 0);
    assert_eq!(mr_after.greeks.delta, 0);
}

// =======================================================================
// Scenario 10: PnL flows through to equity correctly
// =======================================================================

#[test]
fn test_scenario_pnl_equity_flow() {
    let mut acc = SimAccount::new(5_000_000_000); // $5000

    // Open long 20 SOL at $100
    acc.open_perp(0, 20_000_000, 100_000_000).unwrap();

    // Price goes to $120 — unrealized PnL = 20 * (120-100) = $400
    let mr1 = acc.compute_margin(&[120_000_000], 0, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);
    assert_eq!(mr1.equity, 5_400_000_000); // $5000 + $400

    // Price drops to $90 — unrealized PnL = 20 * (90-100) = -$200
    let mr2 = acc.compute_margin(&[90_000_000], 100, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);
    assert_eq!(mr2.equity, 4_800_000_000); // $5000 - $200

    // Close at $90 — realize the -$200 loss
    let pnl = acc.close_perp(0, 90_000_000).unwrap();
    assert_eq!(pnl, -200_000_000);
    assert_eq!(acc.collateral, 4_800_000_000);
    assert_eq!(acc.total_realized_pnl, -200_000_000);
}

// =======================================================================
// Scenario 11: Option premium deduction
// =======================================================================

#[test]
fn test_scenario_option_premium_flows() {
    let mut acc = SimAccount::new(10_000_000_000); // $10,000

    // Buy 10 calls at $5 premium each → pay $50
    acc.open_option(
        0, OptionSide::Call,
        10_000_000,  // buy 10
        150_000_000, 5_000_000,
        500_000, 50_000, 100_000, -10_000,
        1_000_000, 1_000_000,
    ).unwrap();
    // Premium: 10 * $5 / 1e6 * 1e6 = $50
    assert_eq!(acc.collateral, 9_950_000_000);

    // Write 5 puts at $4 premium each → receive $20
    acc.open_option(
        0, OptionSide::Put,
        -5_000_000, // write 5
        140_000_000, 4_000_000,
        -400_000, 40_000, 80_000, -8_000,
        1_000_000, 1_000_000,
    ).unwrap();
    // Premium received: 5 * $4 / 1e6 * 1e6 = $20
    assert_eq!(acc.collateral, 9_970_000_000);
}

// =======================================================================
// Scenario 12: Multiple markets
// =======================================================================

#[test]
fn test_scenario_multi_market() {
    let mut acc = SimAccount::new(50_000_000_000); // $50,000

    // Long SOL perp (market 0)
    acc.open_perp(0, 100_000_000, 150_000_000).unwrap();

    // Short BTC perp (market 1)
    acc.open_perp(1, -2_000_000, 60_000_000_000).unwrap();

    let prices = vec![150_000_000u64, 60_000_000_000u64]; // SOL, BTC
    let mr = acc.compute_margin(&prices, 0, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);

    // SOL delta: +100, BTC delta: -2
    // These are different assets so don't net against each other
    // But total_notional includes both
    assert!(mr.greeks.total_notional > 0);
    assert_eq!(mr.health, AccountHealth::Healthy);
}

// =======================================================================
// Scenario 13: Equity computation with lending
// =======================================================================

#[test]
fn test_scenario_lending_equity() {
    let mut acc = SimAccount::new(10_000_000_000); // $10,000

    // Supply $5000 (adds to equity after haircut)
    acc.add_lending(LendingSide::Supply, 5_000_000_000, 4_750_000_000, 500).unwrap();

    // Borrow $2000 (subtracts from equity)
    acc.add_lending(LendingSide::Borrow, 2_000_000_000, 2_000_000_000, 800).unwrap();

    let mr = acc.compute_margin(&[], 0, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);

    // equity = $10000 + $4750 (supply) - $2000 (borrow) = $12750
    assert_eq!(mr.equity, 12_750_000_000);
}

// =======================================================================
// Scenario 14: Interest accrual changes lending contribution
// =======================================================================

#[test]
fn test_scenario_interest_accrual() {
    // $1000 at 10% APY for 1 year
    let interest = tensor_math::accrue_interest(1_000_000_000, 1000, 31_557_600);
    // ~$100
    assert!((interest as i64 - 100_000_000).abs() < 1_000_000);

    // $1000 at 5% APY for 30 days
    let interest_30d = tensor_math::accrue_interest(1_000_000_000, 500, 2_592_000);
    // ~$4.11
    assert!(interest_30d > 3_000_000);
    assert!(interest_30d < 5_000_000);
}

// =======================================================================
// Scenario 15: Northtail AMM swap math
// =======================================================================

#[test]
fn test_scenario_amm_swap_round_trip() {
    // Pool: 10000 SOL / $1,500,000 USDC
    let security_reserve = 10_000_000_000u64;
    let quote_reserve = 1_500_000_000_000u64;

    // Buy 100 SOL (sell USDC)
    let (sol_out, fee1) = tensor_cpi::northtail::calculate_swap_output(
        15_000_000_000, // $15,000 USDC input
        quote_reserve,
        security_reserve,
        30,
    ).unwrap();

    assert!(sol_out > 0);
    assert!(sol_out < 100_000_000); // Must be less than 100 SOL (slippage)
    assert!(fee1 > 0);

    // Sell back the SOL we got
    let new_security = security_reserve - sol_out;
    let new_quote = quote_reserve + 15_000_000_000 - fee1; // approximate

    let (usdc_back, _fee2) = tensor_cpi::northtail::calculate_swap_output(
        sol_out,
        new_security,
        new_quote,
        30,
    ).unwrap();

    // Round trip should lose money due to fees + slippage
    assert!(usdc_back < 15_000_000_000);
}

// =======================================================================
// Phase 3: Intent and ZK Credit Scenarios
// =======================================================================

// =======================================================================
// Scenario 16: ZK credit discount → lower initial margin
// =======================================================================

#[test]
fn test_scenario_zk_credit_margin_discount() {
    let mut acc = SimAccount::new(10_000_000_000); // $10,000

    // Open long 10 SOL at $150
    acc.open_perp(0, 10_000_000, 150_000_000).unwrap();

    // Compute margin without credit
    let mr_no_credit = acc.compute_margin(
        &[150_000_000], 0, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL,
    );
    assert_eq!(mr_no_credit.initial_margin, 150_000_000); // $150

    // Apply Platinum discount (20%)
    let discounted = tensor_math::apply_credit_discount(
        mr_no_credit.initial_margin,
        ZkCreditTier::Platinum.margin_discount_bps(),
        mr_no_credit.maintenance_margin,
    );

    // 150 * 80% = 120, but floored at maintenance (75)
    assert_eq!(discounted, 120_000_000);
    assert!(discounted < mr_no_credit.initial_margin);
    assert!(discounted >= mr_no_credit.maintenance_margin);
}

// =======================================================================
// Scenario 17: ZK credit + leverage bonus
// =======================================================================

#[test]
fn test_scenario_zk_credit_leverage_bonus() {
    // Retail: 5x base
    let base = InvestorCategory::Retail.max_leverage_bps();
    assert_eq!(base, 50_000);

    // Platinum bonus: +1x → 6x
    let effective = tensor_math::effective_max_leverage_bps(
        base,
        ZkCreditTier::Platinum.leverage_bonus_bps(),
    );
    assert_eq!(effective, 60_000); // 6x

    // Gold bonus: +0.75x → 5.75x
    let effective_gold = tensor_math::effective_max_leverage_bps(
        base,
        ZkCreditTier::Gold.leverage_bonus_bps(),
    );
    assert_eq!(effective_gold, 57_500); // 5.75x

    // Institutional with Platinum: 50x + 1x = 51x
    let inst_base = InvestorCategory::Institutional.max_leverage_bps();
    let inst_effective = tensor_math::effective_max_leverage_bps(
        inst_base,
        ZkCreditTier::Platinum.leverage_bonus_bps(),
    );
    assert_eq!(inst_effective, 510_000); // 51x
}

// =======================================================================
// Scenario 18: Credit tier from score boundaries
// =======================================================================

#[test]
fn test_scenario_credit_tier_progression() {
    // Score progression unlocks tiers
    assert_eq!(ZkCreditTier::from_score(0), ZkCreditTier::None);
    assert_eq!(ZkCreditTier::from_score(300), ZkCreditTier::Bronze);
    assert_eq!(ZkCreditTier::from_score(500), ZkCreditTier::Silver);
    assert_eq!(ZkCreditTier::from_score(650), ZkCreditTier::Gold);
    assert_eq!(ZkCreditTier::from_score(800), ZkCreditTier::Platinum);

    // Each tier increases discount
    let none_disc = ZkCreditTier::None.margin_discount_bps();
    let bronze_disc = ZkCreditTier::Bronze.margin_discount_bps();
    let silver_disc = ZkCreditTier::Silver.margin_discount_bps();
    let gold_disc = ZkCreditTier::Gold.margin_discount_bps();
    let plat_disc = ZkCreditTier::Platinum.margin_discount_bps();

    assert!(none_disc < bronze_disc);
    assert!(bronze_disc < silver_disc);
    assert!(silver_disc < gold_disc);
    assert!(gold_disc < plat_disc);
}

// =======================================================================
// Scenario 19: Intent bundle validation (via tensor-intents)
// =======================================================================

#[test]
fn test_scenario_intent_bundle_creation() {
    // Market buy perp
    let bundle = tensor_intents::market_buy_perp(0, 10_000_000);
    assert!(bundle.validate().is_ok());
    assert_eq!(bundle.leg_count(), 1);

    // Delta-neutral spread
    let dn = tensor_intents::delta_neutral_perp_spot(0, 100_000_000);
    assert!(dn.validate().is_ok());
    assert_eq!(dn.leg_count(), 2);

    // Covered call
    let cc = tensor_intents::covered_call(0, 10_000_000, 160_000_000);
    assert!(cc.validate().is_ok());
    assert_eq!(cc.leg_count(), 2);
}

// =======================================================================
// Scenario 20: Solver decomposes and simulates intent
// =======================================================================

#[test]
fn test_scenario_solver_intent_simulation() {
    // Create a delta-neutral intent
    let bundle = tensor_intents::delta_neutral_perp_spot(0, 100_000_000);
    let prices = vec![150_000_000u64];

    // Decompose
    let steps = tensor_solver::decompose_intent(&bundle, &prices);
    assert_eq!(steps.len(), 2);

    // Simulate with sufficient collateral
    let greeks = PortfolioGreeks::default();
    let result = tensor_solver::simulate_margin_impact(
        &steps,
        &greeks,
        50_000_000_000, // $50,000 collateral
        &tensor_solver::MarginSimConfig::default(),
    );

    assert!(result.feasible);
    assert!(result.estimated_total_margin > 0); // peak from first leg
}

// =======================================================================
// Scenario 21: Solver with credit discount reduces margin
// =======================================================================

#[test]
fn test_scenario_solver_credit_discount() {
    let bundle = tensor_intents::market_buy_perp(0, 100_000_000);
    let prices = vec![150_000_000u64];
    let steps = tensor_solver::decompose_intent(&bundle, &prices);
    let greeks = PortfolioGreeks::default();

    // Without credit
    let result_no = tensor_solver::simulate_margin_impact(
        &steps,
        &greeks,
        50_000_000_000,
        &tensor_solver::MarginSimConfig::default(),
    );

    // With Platinum credit
    let result_with = tensor_solver::simulate_margin_impact(
        &steps,
        &greeks,
        50_000_000_000,
        &tensor_solver::MarginSimConfig {
            credit_discount_bps: 2000,
            ..Default::default()
        },
    );

    assert!(result_with.estimated_total_margin < result_no.estimated_total_margin);
}

// =======================================================================
// Scenario 22: Solver optimization orders hedging legs first
// =======================================================================

#[test]
fn test_scenario_solver_execution_order() {
    // Portfolio is long, so short legs should execute first
    let greeks = PortfolioGreeks {
        delta: 100_000_000,
        ..Default::default()
    };

    let bundle = tensor_intents::IntentBundle::new()
        .add_leg(IntentLeg {
            product_type: ProductType::Perpetual,
            market_index: 0,
            size: 50_000_000, // adds to delta
            limit_price: 150_000_000,
            is_active: true,
        })
        .add_leg(IntentLeg {
            product_type: ProductType::Perpetual,
            market_index: 0,
            size: -80_000_000, // reduces delta
            limit_price: 150_000_000,
            is_active: true,
        });

    let prices = vec![150_000_000u64];
    let mut steps = tensor_solver::decompose_intent(&bundle, &prices);
    tensor_solver::optimize_execution_order(&mut steps, &greeks);

    // Short leg should be first (reduces long delta)
    assert!(steps[0].size < 0);
    assert!(steps[1].size > 0);
}

// =======================================================================
// Scenario 23: Credit-adjusted liquidation threshold
// =======================================================================

#[test]
fn test_scenario_credit_adjusted_liquidation() {
    let mut acc = SimAccount::new(200_000_000); // $200

    // Open long 10 SOL at $150 → $1500 notional
    acc.open_perp(0, 10_000_000, 150_000_000).unwrap();

    // At $150: initial margin = $150, maintenance = $75
    let mr = acc.compute_margin(&[150_000_000], 0, IM_BPS, MAINT_RATIO, GAMMA_BPS, VEGA_BPS, IMPLIED_VOL);
    assert_eq!(mr.initial_margin, 150_000_000);
    assert_eq!(mr.maintenance_margin, 75_000_000);
    assert_eq!(mr.health, AccountHealth::Healthy);

    // With Platinum credit: initial margin discounted by 20%
    // 150 * 80% = 120, floored at maintenance 75
    let discounted = tensor_math::apply_credit_discount(
        mr.initial_margin,
        ZkCreditTier::Platinum.margin_discount_bps(),
        mr.maintenance_margin,
    );
    assert_eq!(discounted, 120_000_000); // $120 vs $150

    // The lower initial margin requirement means positions that would
    // fail the initial margin check without credit now pass with credit
    // A trader with $125 equity would fail at $150 IM but pass at $120 IM
    assert!(125_000_000 < mr.initial_margin as i64);   // fails without credit
    assert!(125_000_000 >= discounted as i64);          // passes with credit
}
